//! The skirmish setup screen widgets, laid over the real `SkirmishGameOptionsMenu.wnd`
//! background (`SkirmishGameOptionsMenu.cpp`, `GUIUtil.cpp`). Functional rather than
//! pixel-perfect: combos cycle on click, the map list is a scrollable overlay.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::gadget;
use crate::shell::{self, cash_label, color_label, faction_label, scaled, ImageEntry, PendingGame, ShellConfig, ShellData, ShellState, SlotState};

/// Marks every entity of the current menu build (despawned on rebuild).
#[derive(Component)]
pub struct MenuRoot;

#[derive(Clone, Copy)]
enum Field {
    Player,
    Faction,
    Color,
    Team,
}

/// Every clickable menu element. One component + one query keeps `menu_input` under Bevy's
/// 16-parameter system limit.
#[derive(Component, Clone, Copy)]
enum MenuAction {
    Slot { slot: usize, field: Field },
    Start,
    Back,
    SelectMap,
    CloseMap,
    MapRow(usize),
    MapScroll(i32),
    StartMarker(i32),
    Cash,
    Sw,
    Resume,
    Restart,
    Quit,
    Options,
}

/// Set when the menu must be rebuilt (on enter, or after a selection change).
#[derive(Resource, Default)]
pub struct MenuDirty(pub bool);

/// Set when the pause overlay must be rebuilt.
#[derive(Resource, Default)]
pub struct PauseDirty(pub bool);

#[derive(Component)]
struct PauseRoot;
#[derive(Component)]
struct ScoreRoot;

pub fn add_systems(app: &mut App) {
    app.init_resource::<MenuDirty>();
    app.init_resource::<PauseDirty>();
    app.add_systems(OnEnter(ShellState::Setup), mark_dirty);
    app.add_systems(OnExit(ShellState::Setup), despawn_menu);
    app.add_systems(Update, (menu_build, menu_input).run_if(in_state(ShellState::Setup)));
    app.add_systems(Update, (pause_build, pause_click).run_if(in_state(ShellState::InGame)));
    app.add_systems(OnEnter(ShellState::Score), score_enter);
    app.add_systems(OnExit(ShellState::Score), score_exit);
    app.add_systems(Update, score_click.run_if(in_state(ShellState::Score)));
}

/// Builds the pause menu when [`PauseDirty`] is set: `Menus/QuitMenu.wnd` (the in-game menu, with
/// `QuitMenu.cpp`'s buttons), or a plain panel when that file is missing.
#[allow(clippy::too_many_arguments)]
fn pause_build(
    mut commands: Commands,
    mut dirty: ResMut<PauseDirty>,
    pause: Res<crate::PauseMenu>,
    shell: Option<Res<ShellData>>,
    window: Query<&Window, With<PrimaryWindow>>,
    existing: Query<Entity, With<PauseRoot>>,
) {
    if !dirty.0 {
        return;
    }
    dirty.0 = false;
    for e in &existing {
        commands.entity(e).despawn();
    }
    if !pause.0 {
        return;
    }
    // The `.wnd` needs the shell's decoded images; a `--quick` game has none and keeps the panel.
    if let Some(shell) = shell.as_deref().filter(|s| s.quit_wnd.is_some()) {
        if let Some(wnd) = &shell.quit_wnd {
            pause_wnd(&mut commands, wnd, shell, window.single().ok());
            return;
        }
    }
    let root = overlay_panel(&mut commands, "Paused", 360.0, 260.0, PauseRoot);
    spawn_button_in(&mut commands, root, 140.0, "Resume", MenuAction::Resume);
    spawn_button_in(&mut commands, root, 140.0, "Restart", MenuAction::Restart);
    spawn_button_in(&mut commands, root, 140.0, "Options", MenuAction::Options);
    spawn_button_in(&mut commands, root, 140.0, "Quit to Setup", MenuAction::Quit);
}

/// Draws `Menus/QuitMenu.wnd` (`QuitMenu.cpp` `QuitMenuInit`) over the game view, with its buttons
/// wired to the pause actions: Return to Game (`ButtonReturn`), Restart (`ButtonRestart`),
/// Options (`ButtonOptions`) and Exit (`ButtonExit`, which quits to the setup screen here).
/// `ButtonSaveLoad` only highlights: there is no save system yet.
fn pause_wnd(commands: &mut Commands, wnd: &zr_ui::wnd::WndFile, shell: &ShellData, win: Option<&Window>) {
    let root = commands
        .spawn((
            PauseRoot,
            Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
        ))
        .id();
    let scene = gadget::Scene {
        wnd,
        images: &shell.images,
        fonts: &shell.fonts,
        text: &shell.text,
        window: win,
        hide: &|_: &str| false,
        reveal: &|_: &str| false,
        label: &|_: &str| None::<String>,
        checked: &|_: &str| None::<bool>,
        tint: &|_: &str| None::<Color>,
        fraction: &|_: &str| None::<f32>,
    };
    for (name, entity) in gadget::render(commands, root, &scene) {
        let short = name.rsplit(':').next().unwrap_or("");
        let action = match short {
            "ButtonReturn" => Some(MenuAction::Resume),
            "ButtonRestart" => Some(MenuAction::Restart),
            "ButtonOptions" => Some(MenuAction::Options),
            "ButtonExit" => Some(MenuAction::Quit),
            _ => None,
        };
        match action {
            Some(action) => gadget::make_clickable(commands, entity, action),
            None => gadget::make_hoverable(commands, entity),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn pause_click(
    mut commands: Commands,
    mut pause: ResMut<crate::PauseMenu>,
    mut paused: ResMut<crate::Paused>,
    mut next: ResMut<NextState<ShellState>>,
    current: Option<Res<crate::CurrentGame>>,
    mut dirty: ResMut<PauseDirty>,
    mut options: ResMut<crate::options::OptionsState>,
    actions: Query<(&Interaction, &MenuAction), Changed<Interaction>>,
) {
    for (interaction, action) in &actions {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match action {
            MenuAction::Options => {
                options.open = true;
                options.dirty = true;
                pause.0 = false;
                paused.0 = false;
                dirty.0 = true;
            }
            MenuAction::Resume => {
                pause.0 = false;
                paused.0 = false;
                dirty.0 = true;
            }
            MenuAction::Restart => {
                if let Some(current) = &current {
                    commands.insert_resource(PendingGame(current.0.clone()));
                    next.set(ShellState::Restart);
                }
            }
            MenuAction::Quit => {
                pause.0 = false;
                paused.0 = false;
                dirty.0 = true;
                next.set(ShellState::Setup);
            }
            _ => {}
        }
    }
}

/// `OnExit(Score)`: the score screen goes when the state changes (the next screen must not show it).
fn score_exit(mut commands: Commands, existing: Query<Entity, With<ScoreRoot>>) {
    for e in &existing {
        commands.entity(e).despawn();
    }
}

/// Builds the score screen: `Menus/ScoreScreen.wnd` when the shell data is loaded, else the plain
/// panel (a `--quick` game has no shell data).
fn score_enter(
    mut commands: Commands,
    result: Res<crate::GameResult>,
    current: Option<Res<crate::CurrentGame>>,
    shell: Option<Res<ShellData>>,
    sim: Option<NonSend<crate::sim_view::SimRes>>,
    window: Query<&Window, With<PrimaryWindow>>,
    existing: Query<Entity, With<ScoreRoot>>,
) {
    for e in &existing {
        commands.entity(e).despawn();
    }
    if let Some(shell) = shell.as_deref() {
        if let Some(wnd) = &shell.score_wnd {
            let rows = match (sim.as_deref(), current.as_deref()) {
                (Some(sim), Some(c)) => score_rows(&sim.game, c.0.slots.len()),
                _ => Vec::new(),
            };
            score_wnd(&mut commands, wnd, shell, current.as_deref().map(|c| &c.0), &rows, window.single().ok());
            return;
        }
    }
    let title = if result.victory { "Victory" } else { "Defeat" };
    let root = overlay_panel(&mut commands, title, 460.0, 260.0, ScoreRoot);
    let secs = result.frame / 30;
    let time = format!("Time: {:02}:{:02}", secs / 60, secs % 60);
    spawn_label_in(&mut commands, root, &time, 16.0, Color::srgb(0.9, 0.9, 0.9));
    if let Some(c) = &current {
        spawn_label_in(&mut commands, root, &c.0.map, 14.0, Color::srgb(0.8, 0.8, 0.8));
    }
    spawn_button_in(&mut commands, root, 180.0, "Quit to Setup", MenuAction::Quit);
}

/// The stat columns of one skirmish row (`populatePlayerInfo`), by control name.
const STAT_CONTROLS: [&str; 7] = [
    "StaticTextUnitsBuilt",
    "StaticTextUnitsLost",
    "StaticTextUnitsDestroyed",
    "StaticTextBuildingsBuilt",
    "StaticTextBuildingsLost",
    "StaticTextBuildingsDestroyed",
    "StaticTextResources",
];

/// The row a score-screen row control belongs to (`StaticTextUnitsLost3` -> 3); `None` for any other control.
fn score_row_of(control: &str) -> Option<usize> {
    let stem = control.trim_end_matches(|c: char| c.is_ascii_digit());
    let is_row = stem.len() < control.len() && (stem == "StaticTextPlayer" || STAT_CONTROLS.contains(&stem));
    if is_row {
        control[stem.len()..].parse().ok()
    } else {
        None
    }
}

/// One row of the skirmish score screen: the slot (its player's name and colour) and the player's
/// keeper, `None` when the slot has no player in the sim.
struct ScoreRow {
    slot: usize,
    keeper: Option<zr_sim::score::ScoreKeeper>,
}

/// `grabMultiPlayerInfo` (ScoreScreen.cpp): the rows best score first. Each player's `calculateScore`
/// is a key in a map; a key already taken gets `adder` added (and `adder` counts up), as in C++.
fn score_rows(game: &zr_game::Game, slots: usize) -> Vec<ScoreRow> {
    let mut scores: std::collections::BTreeMap<u64, ScoreRow> = std::collections::BTreeMap::new();
    let mut adder: u64 = 1;
    for slot in 0..slots {
        let keeper = game.skirmish_player(slot).and_then(|p| game.logic().score_keeper(p.index)).cloned();
        let mut score = keeper.as_ref().map_or(0, |k| k.calculate_score());
        if scores.contains_key(&score) {
            score += adder;
            adder += 1;
        }
        scores.insert(score, ScoreRow { slot, keeper });
    }
    scores.into_values().rev().collect()
}

/// Draws `Menus/ScoreScreen.wnd` (`ScoreScreen.cpp`) over the game view. Row `n` (`StaticTextPlayer{n}`
/// and its stat controls) belongs to the `n`th best player by `calculateScore` (`grabMultiPlayerInfo`):
/// the name (the human's machine name or the AI's army) and the stats are in the player's slot colour
/// (`populatePlayerInfo`). The score itself has no text (C++ comments that control out for skirmish).
/// `ButtonOk` (EXIT) returns to the setup screen; `ButtonSaveReplay` only highlights.
fn score_wnd(
    commands: &mut Commands,
    wnd: &zr_ui::wnd::WndFile,
    shell: &ShellData,
    game: Option<&shell::GameConfig>,
    rows: &[ScoreRow],
    win: Option<&Window>,
) {
    let root = commands
        .spawn((
            ScoreRoot,
            Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
        ))
        .id();
    let mut labels: HashMap<String, String> = HashMap::new();
    let mut tints: HashMap<String, Color> = HashMap::new();
    let slots = game.map_or(&[][..], |g| g.slots.as_slice());
    for (pos, row) in rows.iter().enumerate() {
        let Some(slot) = slots.get(row.slot) else { continue };
        let colour = usize::try_from(slot.color).ok().and_then(|c| shell.colors.get(c)).map(|c| c.rgb).map(|rgb| {
            Color::srgb(rgb[0] as f32 / 255.0, rgb[1] as f32 / 255.0, rgb[2] as f32 / 255.0)
        });
        let name = if slot.is_ai { shell::ai_army_label(&shell.text, slot.difficulty) } else { shell::human_name() };
        let Some(k) = row.keeper.as_ref() else { continue };
        let stats = [
            k.total_units_built(),
            k.total_units_lost(),
            k.total_units_destroyed(),
            k.total_buildings_built(),
            k.total_buildings_lost(),
            k.total_buildings_destroyed(),
            k.total_money_earned(),
        ];
        let mut put = |control: String, text: String| {
            if let Some(c) = colour {
                tints.insert(control.clone(), c);
            }
            labels.insert(control, text);
        };
        put(format!("StaticTextPlayer{pos}"), name);
        for (control, value) in STAT_CONTROLS.iter().zip(stats) {
            put(format!("{control}{pos}"), value.to_string());
        }
    }
    // The rows past the last player are hidden (`hideWindows`).
    let row_count = rows.len();
    let hidden_row = |s: &str| score_row_of(s).is_some_and(|r| r >= row_count);
    let scene = gadget::Scene {
        wnd,
        images: &shell.images,
        fonts: &shell.fonts,
        text: &shell.text,
        window: win,
        // The multiplayer-only chat, buddy and emote controls, the observer labels and the winner
        // flags (no flag art is set per player yet) are not part of the single-player screen.
        hide: &|name: &str| {
            let s = name.rsplit(':').next().unwrap_or("");
            ["ButtonBuddy", "ButtonEmote", "ButtonContinue", "ListboxChatWindowScoreScreen"].contains(&s)
                || s.contains("Chat")
                || s.starts_with("StaticTextObserver")
                || s.starts_with("GameWindowWinner")
                || hidden_row(s)
        },
        reveal: &|_: &str| false,
        label: &|name: &str| labels.get(name.rsplit(':').next().unwrap_or("")).cloned(),
        checked: &|_: &str| None::<bool>,
        tint: &|name: &str| tints.get(name.rsplit(':').next().unwrap_or("")).copied(),
        fraction: &|_: &str| None::<f32>,
    };
    for (name, entity) in gadget::render(commands, root, &scene) {
        match name.rsplit(':').next().unwrap_or("") {
            // `ButtonOk` is the bottom-right EXIT (retail's score screen has only SAVE REPLAY and EXIT).
            "ButtonOk" => gadget::make_clickable(commands, entity, MenuAction::Quit),
            _ => gadget::make_hoverable(commands, entity),
        }
    }
}

fn score_click(mut next: ResMut<NextState<ShellState>>, actions: Query<(&Interaction, &MenuAction), Changed<Interaction>>) {
    for (interaction, action) in &actions {
        if *interaction == Interaction::Pressed && matches!(action, MenuAction::Quit) {
            next.set(ShellState::Setup);
        }
    }
}

/// A centered dark panel with a title; `marker` identifies the root for rebuilds.
fn overlay_panel<M: Component>(commands: &mut Commands, title: &str, w: f32, h: f32, marker: M) -> Entity {
    let root = commands
        .spawn((
            marker,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
        ))
        .id();
    let panel = commands
        .spawn((
            Node {
                width: Val::Px(w),
                height: Val::Px(h),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(10.0),
                border: UiRect::all(Val::Px(2.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.06, 0.07, 0.1, 0.98)),
            BorderColor::all(Color::srgb(0.6, 0.55, 0.3)),
            ChildOf(root),
        ))
        .id();
    commands.spawn((
        Text::new(title.to_owned()),
        TextFont { font_size: FontSize::Px(30.0), ..default() },
        TextColor(Color::srgb(1.0, 0.9, 0.5)),
        ChildOf(panel),
    ));
    panel
}

fn spawn_label_in(commands: &mut Commands, parent: Entity, text: &str, size: f32, color: Color) {
    commands.spawn((
        Text::new(text.to_owned()),
        TextFont { font_size: FontSize::Px(size), ..default() },
        TextColor(color),
        ChildOf(parent),
    ));
}

fn mark_dirty(mut dirty: ResMut<MenuDirty>) {
    dirty.0 = true;
}

/// `OnExit(Setup)`: make sure the menu entities are gone even when the state changed before
/// `menu_build` ran (the `--auto-start` path), so the setup screen cannot leak over the game.
fn despawn_menu(mut commands: Commands, existing: Query<Entity, With<MenuRoot>>) {
    for e in &existing {
        commands.entity(e).despawn();
    }
}

/// Rebuilds the menu when [`MenuDirty`] is set.
#[allow(clippy::too_many_arguments)]
fn menu_build(
    mut commands: Commands,
    mut dirty: ResMut<MenuDirty>,
    shell: Res<ShellData>,
    config: Res<ShellConfig>,
    window: Query<&Window, With<PrimaryWindow>>,
    existing: Query<Entity, With<MenuRoot>>,
) {
    if !dirty.0 {
        return;
    }
    dirty.0 = false;
    for e in &existing {
        commands.entity(e).despawn();
    }
    let win = window.single().ok();
    let root = commands
        .spawn((
            MenuRoot,
            Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
        ))
        .id();
    build_setup_gadgets(&mut commands, root, &shell, &config, win);
    build_widgets(&mut commands, root, &shell, &config, win);
    if config.show_map_list {
        build_map_list(&mut commands, root, &shell, &config, win);
    }
}

/// Looks up a `.wnd` window's scaled rect.
fn rect_of(shell: &ShellData, win: Option<&Window>, name: &str) -> Option<(f32, f32, f32, f32)> {
    let wnd = shell.wnd.as_ref()?;
    let (sx, sy) = shell::wnd_scale(wnd, win);
    wnd.windows
        .iter()
        .flat_map(|r| r.walk())
        .find(|w| w.name.as_deref() == Some(name))
        .map(|w| scaled(w.rect, sx, sy))
}

/// `sx, sy` for the fallback layout (creation 800x600 -> window).
fn fallback_scale(win: Option<&Window>) -> (f32, f32) {
    let (w, h) = win.map_or((1600.0, 900.0), |w| (w.width(), w.height()));
    (w / 800.0, h / 600.0)
}

fn build_widgets(commands: &mut Commands, root: Entity, shell: &ShellData, config: &ShellConfig, win: Option<&Window>) {
    let (fsx, fsy) = fallback_scale(win);
    let fallback = |x: f32, y: f32, w: f32, h: f32| (x * fsx, y * fsy, w * fsx, h * fsy);

    // Map preview in `MapWindow`, with the engine's start-position markers (`positionStartSpots`).
    let map_rect = rect_of(shell, win, "SkirmishGameOptionsMenu.wnd:MapWindow").unwrap_or_else(|| fallback(30.0, 90.0, 300.0, 220.0));
    if let Some(map) = shell.maps.get(config.map) {
        let mut node = commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(map_rect.0),
                top: Val::Px(map_rect.1),
                width: Val::Px(map_rect.2),
                height: Val::Px(map_rect.3),
                ..default()
            },
            ImageNode::default(),
            ChildOf(root),
        ));
        if let Some(p) = &map.preview {
            node.insert(ImageNode { image: p.clone(), ..default() });
        }
        let ew = map.extent_max[0] - map.extent_min[0];
        let eh = map.extent_max[1] - map.extent_min[1];
        if ew > 1.0 && eh > 1.0 {
            // `findDrawPositions`: letterbox the extent into the map window.
            let (ratio_w, ratio_h) = (ew / map_rect.2.max(1.0), eh / map_rect.3.max(1.0));
            let (rw, rh, ulx, uly) = if ratio_w >= ratio_h {
                let (rw, rh) = (ew / ratio_w, eh / ratio_w);
                (rw, rh, 0.0, (map_rect.3 - rh) / 2.0)
            } else {
                let (rw, rh) = (ew / ratio_h, eh / ratio_h);
                (rw, rh, (map_rect.2 - rw) / 2.0, 0.0)
            };
            // The marker is the `PlayerStart` art (the loading screen's marker) at the `.wnd`
            // start button's size, numbered with the slot that owns the spot.
            let size = marker_size(shell, win);
            for (p, start) in map.starts.iter().enumerate() {
                let fx = (start[0] - map.extent_min[0]) / ew;
                let fy = (start[1] - map.extent_min[1]) / eh;
                let rect = (map_rect.0 + fx * rw + ulx - size.0 / 2.0, map_rect.1 + (1.0 - fy) * rh + uly - size.1 / 2.0, size.0, size.1);
                let owner = config.slots.iter().position(|s| s.state.is_active() && s.start == p as i32);
                let label = owner.map(|i| (i + 1).to_string()).unwrap_or_default();
                spawn_start_marker(commands, root, rect, &label, MenuAction::StartMarker(p as i32), shell.images.get("PlayerStart"), owner.is_some());
            }
        }
    }
}

/// The setup screen's `.wnd` gadgets (`SkirmishGameOptionsMenu.wnd`): every slot combo, the map
/// name, the select / cash / superweapon / start / back controls are drawn with their own art
/// (`gadget.rs`), labelled with the current value, and wired to their action. The `.wnd` game-speed
/// slider stays out (the sim runs at a fixed 30 Hz).
fn build_setup_gadgets(commands: &mut Commands, root: Entity, shell: &ShellData, config: &ShellConfig, win: Option<&Window>) {
    let Some(wnd) = &shell.wnd else { return };
    // Row 0 is the human's name entry (`TextEntryPlayerName`); rows 1..7 are `ComboBoxPlayer{n}`.
    let mut labels: HashMap<String, String> = HashMap::new();
    let mut actions: HashMap<String, MenuAction> = HashMap::new();
    let mut tints: HashMap<String, Color> = HashMap::new();
    let mut checks: HashMap<String, bool> = HashMap::new();
    for (i, s) in config.slots.iter().enumerate() {
        let player = if i == 0 { "TextEntryPlayerName".to_owned() } else { format!("ComboBoxPlayer{i}") };
        // The slot's colour names the player and its army in that colour (`Red`, `Green`, the
        // `Medium Army` name) as the engine's setup list does; `Random` and `???` keep the default.
        if let Some(rgb) = usize::try_from(s.color).ok().and_then(|c| shell.colors.get(c)).map(|c| c.rgb) {
            let tint = Color::srgb(rgb[0] as f32 / 255.0, rgb[1] as f32 / 255.0, rgb[2] as f32 / 255.0);
            tints.insert(player.clone(), tint);
            tints.insert(format!("ComboBoxColor{i}"), tint);
        }
        labels.insert(player.clone(), slot_label(shell, s.state));
        actions.insert(player, MenuAction::Slot { slot: i, field: Field::Player });
        labels.insert(format!("ComboBoxColor{i}"), color_label(shell, s.color));
        actions.insert(format!("ComboBoxColor{i}"), MenuAction::Slot { slot: i, field: Field::Color });
        labels.insert(format!("ComboBoxPlayerTemplate{i}"), faction_label(shell, s.faction));
        actions.insert(format!("ComboBoxPlayerTemplate{i}"), MenuAction::Slot { slot: i, field: Field::Faction });
        labels.insert(format!("ComboBoxTeam{i}"), team_label(shell, s.team));
        actions.insert(format!("ComboBoxTeam{i}"), MenuAction::Slot { slot: i, field: Field::Team });
    }
    let cash = shell.cash_choices.get(config.starting_cash).copied().unwrap_or(0);
    labels.insert("ComboBoxStartingCash".into(), cash_label(&shell.text, cash));
    actions.insert("ComboBoxStartingCash".into(), MenuAction::Cash);
    labels.insert("TextEntryMapDisplay".into(), shell::map_display(shell, config.map));
    actions.insert("ButtonSelectMap".into(), MenuAction::SelectMap);
    // The check box keeps its `.wnd` label (`GUI:LimitSuperweapons`) and draws its box by state.
    checks.insert("CheckboxLimitSuperweapons".into(), config.superweapons);
    actions.insert("CheckboxLimitSuperweapons".into(), MenuAction::Sw);
    actions.insert("ButtonStart".into(), MenuAction::Start);
    actions.insert("ButtonBack".into(), MenuAction::Back);

    let short = |name: &str| name.rsplit(':').next().unwrap_or("").to_owned();
    // The `.wnd`'s `ButtonMapStartPosition*` sit at their unplaced file rects (a diagonal chain
    // beside the preview); retail moves them onto the start spots, which `build_widgets` draws.
    let hidden = |name: &str| {
        let s = short(name);
        s.starts_with("ButtonMapStartPosition") || matches!(s.as_str(), "StaticTextGameSpeed" | "SliderGameSpeed" | "ButtonReset" | "ButtonResetFPS")
    };
    let scene = gadget::Scene {
        wnd,
        images: &shell.images,
        fonts: &shell.fonts,
        text: &shell.text,
        window: win,
        hide: &hidden,
        reveal: &|_: &str| false,
        label: &|name: &str| labels.get(&short(name)).cloned(),
        checked: &|name: &str| checks.get(&short(name)).copied(),
        tint: &|name: &str| tints.get(&short(name)).copied(),
        fraction: &|_: &str| None::<f32>,
    };
    for (name, entity) in gadget::render(commands, root, &scene) {
        match actions.get(&short(&name)) {
            Some(action) => gadget::make_clickable(commands, entity, *action),
            None => gadget::make_hoverable(commands, entity),
        }
    }
}

fn build_map_list(commands: &mut Commands, root: Entity, shell: &ShellData, config: &ShellConfig, win: Option<&Window>) {
    let (w, h) = win.map_or((1600.0, 900.0), |w| (w.width(), w.height()));
    let panel_w = 520.0f32.min(w - 40.0);
    let panel_h = 460.0f32.min(h - 40.0);
    let left = (w - panel_w) / 2.0;
    let top = (h - panel_h) / 2.0;
    let panel = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(left),
                top: Val::Px(top),
                width: Val::Px(panel_w),
                height: Val::Px(panel_h),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(8.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.05, 0.06, 0.08, 0.98)),
            BorderColor::all(Color::srgb(0.5, 0.5, 0.55)),
            ChildOf(root),
        ))
        .id();
    // Dim the screen behind the panel.
    commands.spawn((
        Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.45)),
        ZIndex(-1),
        ChildOf(root),
    ));
    let row_h = 24.0;
    let visible = ((panel_h - 70.0) / row_h) as usize;
    let start = config.map_scroll.min(shell.maps.len().saturating_sub(1));
    for (row, i) in (start..shell.maps.len()).take(visible).enumerate() {
        let m = &shell.maps[i];
        let label = if m.multiplayer { format!("{}  [{}p]", m.display, m.num_players) } else { m.display.clone() };
        let btn = commands
            .spawn((
                Button,
                Interaction::default(),
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Px(row_h - 2.0),
                    margin: UiRect::bottom(Val::Px(2.0)),
                    ..default()
                },
                BackgroundColor(if i == config.map { Color::srgb(0.25, 0.35, 0.5) } else { Color::srgb(0.12, 0.13, 0.16) }),
                ChildOf(panel),
            ))
            .id();
        commands.entity(btn).insert(MenuAction::MapRow(i));
        spawn_text_child(commands, btn, &label, 13.0, Color::srgb(0.95, 0.95, 0.9));
        let _ = row;
    }
    // Scroll + close row.
    let controls = commands
        .spawn((Node { width: Val::Percent(100.0), height: Val::Px(28.0), margin: UiRect::top(Val::Px(4.0)), ..default() }, ChildOf(panel)))
        .id();
    spawn_button_in(commands, controls, 90.0, "Up", MenuAction::MapScroll(-1));
    spawn_button_in(commands, controls, 90.0, "Down", MenuAction::MapScroll(1));
    spawn_button_in(commands, controls, 90.0, "Close", MenuAction::CloseMap);
}

/// The player-name box text of a slot: the human's name, the AI's army name (`shell::ai_army_label`),
/// or the slot's state.
fn slot_label(shell: &ShellData, state: SlotState) -> String {
    match state {
        SlotState::Human => shell::human_name(),
        SlotState::Open => "Open".into(),
        SlotState::Closed => "Closed".into(),
        SlotState::EasyAi | SlotState::NormalAi | SlotState::HardAi => shell::ai_army_label(&shell.text, state.difficulty()),
    }
}

fn team_label(shell: &ShellData, team: u8) -> String {
    shell::team_labels(&shell.text).get(team as usize).cloned().unwrap_or_else(|| format!("Team {team}"))
}

/// The start marker's size: the `.wnd`'s `ButtonMapStartPosition0` rect (the engine's marker
/// button), or 20 px without the file.
fn marker_size(shell: &ShellData, win: Option<&Window>) -> (f32, f32) {
    rect_of(shell, win, "SkirmishGameOptionsMenu.wnd:ButtonMapStartPosition0").map_or((20.0, 20.0), |r| (r.2, r.3))
}

/// A start marker: the `PlayerStart` art (dimmed while no slot owns the spot) with the owner's
/// number over it. Clickable through `marker` like the other setup controls.
#[allow(clippy::too_many_arguments)]
fn spawn_start_marker<M: Component>(
    commands: &mut Commands,
    root: Entity,
    rect: (f32, f32, f32, f32),
    label: &str,
    marker: M,
    image: Option<&ImageEntry>,
    owned: bool,
) {
    let tint = if owned { Color::WHITE } else { Color::srgba(1.0, 1.0, 1.0, 0.45) };
    let mut node = commands.spawn((
        Button,
        Interaction::default(),
        marker,
        Node { position_type: PositionType::Absolute, left: Val::Px(rect.0), top: Val::Px(rect.1), width: Val::Px(rect.2), height: Val::Px(rect.3), ..default() },
        ChildOf(root),
    ));
    match image {
        Some(img) => {
            node.insert(ImageNode { image: img.handle.clone(), rect: img.rect, image_mode: NodeImageMode::Stretch, color: tint, ..default() });
        }
        None => {
            node.insert(BackgroundColor(if owned { Color::srgba(0.15, 0.35, 0.6, 0.9) } else { Color::srgba(0.1, 0.1, 0.12, 0.55) }));
        }
    }
    let e = node.id();
    if !label.is_empty() {
        spawn_text_child(commands, e, label, 13.0, Color::srgb(1.0, 1.0, 0.7));
    }
}

fn spawn_button_in<M: Component>(commands: &mut Commands, parent: Entity, width: f32, label: &str, marker: M) {
    let e = commands
        .spawn((
            Button,
            Interaction::default(),
            marker,
            Node { width: Val::Px(width), height: Val::Px(24.0), margin: UiRect::right(Val::Px(6.0)), border: UiRect::all(Val::Px(1.0)), ..default() },
            BackgroundColor(Color::srgba(0.14, 0.15, 0.18, 0.95)),
            BorderColor::all(Color::srgb(0.5, 0.5, 0.55)),
            ChildOf(parent),
        ))
        .id();
    spawn_text_child(commands, e, label, 13.0, Color::srgb(0.95, 0.95, 0.9));
}

fn spawn_text_child(commands: &mut Commands, parent: Entity, text: &str, size: f32, color: Color) {
    commands.spawn((
        Text::new(text.to_owned()),
        TextFont { font_size: FontSize::Px(size), ..default() },
        TextColor(color),
        TextLayout { justify: Justify::Center, ..default() },
        Node { width: Val::Percent(100.0), height: Val::Percent(100.0), align_items: AlignItems::Center, justify_content: JustifyContent::Center, ..default() },
        ChildOf(parent),
    ));
}

/// Handles clicks and rebuilds when the selection changes.
#[allow(clippy::too_many_arguments)]
fn menu_input(
    mut commands: Commands,
    mut dirty: ResMut<MenuDirty>,
    shell: Res<ShellData>,
    mut config: ResMut<ShellConfig>,
    mut next: ResMut<NextState<ShellState>>,
    actions: Query<(&Interaction, &MenuAction), Changed<Interaction>>,
    existing: Query<Entity, With<MenuRoot>>,
) {
    let mut changed = false;
    for (interaction, action) in &actions {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match action {
            MenuAction::Slot { slot, field } => {
                let s = &mut config.slots[*slot];
                match field {
                    Field::Player if *slot != 0 => {
                        s.state = match s.state {
                            SlotState::Open => SlotState::Closed,
                            SlotState::Closed => SlotState::EasyAi,
                            SlotState::EasyAi => SlotState::NormalAi,
                            SlotState::NormalAi => SlotState::HardAi,
                            SlotState::HardAi => SlotState::Open,
                            SlotState::Human => SlotState::Open,
                        };
                    }
                    Field::Faction => s.faction = (s.faction + 1) % shell.factions.len().max(1),
                    Field::Color => {
                        let n = shell.colors.len() as i32;
                        s.color = if s.color + 1 >= n { -1 } else { s.color + 1 };
                    }
                    Field::Team => s.team = (s.team + 1) % 5,
                    Field::Player => {}
                }
                changed = true;
            }
            MenuAction::StartMarker(p) => {
                // Clear an owned position, else give it to a free slot (the human first).
                if let Some(owner) = config.slots.iter().position(|s| s.state.is_active() && s.start == *p) {
                    config.slots[owner].start = -1;
                } else if let Some(free) = config.slots.iter().position(|s| s.state.is_active() && s.start == -1) {
                    config.slots[free].start = *p;
                } else {
                    config.slots[0].start = *p;
                }
                changed = true;
            }
            MenuAction::Start => {
                let game = config.to_game(&shell);
                info!("shell: starting {} with {} slots, cash {}", game.map, game.slots.len(), game.starting_cash);
                for e in &existing {
                    commands.entity(e).despawn();
                }
                commands.insert_resource(PendingGame(game));
                next.set(ShellState::Loading);
                return;
            }
            MenuAction::Back => {
                // Back to the start screen (`MainMenu.wnd`); Exit there quits.
                next.set(ShellState::MainMenu);
                return;
            }
            MenuAction::SelectMap => {
                config.show_map_list = true;
                changed = true;
            }
            MenuAction::CloseMap => {
                config.show_map_list = false;
                changed = true;
            }
            MenuAction::MapRow(i) => {
                config.map = *i;
                config.show_map_list = false;
                changed = true;
            }
            MenuAction::MapScroll(step) => {
                let n = shell.maps.len();
                if *step < 0 {
                    config.map_scroll = config.map_scroll.saturating_sub(8);
                } else {
                    config.map_scroll = (config.map_scroll + 8).min(n.saturating_sub(1));
                }
                changed = true;
            }
            MenuAction::Cash => {
                config.starting_cash = (config.starting_cash + 1) % shell.cash_choices.len().max(1);
                changed = true;
            }
            MenuAction::Sw => {
                config.superweapons = !config.superweapons;
                changed = true;
            }
            MenuAction::Resume | MenuAction::Restart | MenuAction::Quit | MenuAction::Options => {}
        }
    }
    if changed {
        dirty.0 = true;
    }
}
