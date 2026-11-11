//! Skirmish load screen (`Window\Menus\MultiplayerLoadScreen.wnd`).
//!
//! `GameLogic::getLoadScreen` returns `MultiPlayerLoadScreen` for `GAME_SKIRMISH`, and
//! `MultiPlayerLoadScreen::init` fills the window from each occupied `GameSlot` (name, side, house
//! colour; the AI progress bars are hidden, `if (slot->isAI()) m_progressBars[netSlot]->winHide(TRUE)`).
//! The viewer shows the same layout with the chosen players while the world is built.
//!
//! The progress bar is driven by the real `GameLogic::updateLoadProgress` `LOAD_PROGRESS_*`
//! checkpoints: [`crate::build`] stages the world build across frames and writes [`LoadProgress`]
//! after each stage. `--load-screen` holds the bar at 40 for the screenshot harness.
//!
//! The `.wnd` is built from `ShellData`, which the menu path loads before entering `Loading`. A
//! direct `--load-screen` start loads it in the same frame (`OnEnter(Loading)`), so the UI is built
//! one frame later in [`build_load_screen`] once the resource exists.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::shell::{self, ShellData, ShellState};

/// The current load-screen progress (0..100), fed by [`crate::build`]'s `LOAD_PROGRESS_*` stages.
#[derive(Resource, Default)]
pub struct LoadProgress(pub u32);

/// Frames spent on the load screen (the `--load-screen` hold).
#[derive(Resource, Default)]
struct LoadWait(u32);

/// Whether the load screen's widgets have been built.
#[derive(Resource, Default)]
struct LoadBuilt(bool);

#[derive(Component)]
struct LoadScreenRoot;
#[derive(Component)]
struct LoadBarFill;

pub fn add_systems(app: &mut App) {
    app.init_resource::<LoadProgress>();
    app.init_resource::<LoadWait>();
    app.init_resource::<LoadBuilt>();
    app.add_systems(OnEnter(ShellState::Loading), start_loading);
    app.add_systems(Update, (build_load_screen, loading_hold, load_bar_refresh).run_if(in_state(ShellState::Loading)));
    app.add_systems(OnExit(ShellState::Loading), stop_loading);
}

/// Resets the bar and waits for `ShellData` before [`build_load_screen`] draws the layout.
fn start_loading(mut progress: ResMut<LoadProgress>, mut wait: ResMut<LoadWait>, mut built: ResMut<LoadBuilt>) {
    wait.0 = 0;
    progress.0 = 0;
    built.0 = false;
}

/// Builds the real `MultiplayerLoadScreen.wnd` once `ShellData` exists, mirroring
/// `MultiPlayerLoadScreen::init` (`LoadScreen.cpp`): it fills the **first `netSlot`** widgets
/// (player / side / team / progress, indexed by occupied order, not by slot), hides the AI progress
/// bars and every unused slot's widgets, shows the map preview in `WinMapPreview`, and leaves the
/// `.wnd` placeholders (which the engine overwrites) undrawn.
#[allow(clippy::too_many_arguments)]
fn build_load_screen(
    mut commands: Commands,
    mut built: ResMut<LoadBuilt>,
    pending: Option<Res<shell::PendingGame>>,
    shell_data: Option<Res<ShellData>>,
    window: Query<&Window, With<PrimaryWindow>>,
    existing: Query<Entity, With<LoadScreenRoot>>,
) {
    if built.0 || existing.iter().next().is_some() {
        return;
    }
    let (Some(pending), Some(shell)) = (pending, shell_data) else { return };
    built.0 = true;
    let config = pending.0.clone();
    let win = window.single().ok();
    let root = commands
        .spawn((
            LoadScreenRoot,
            Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
            ZIndex(600),
        ))
        .id();
    // Widgets the engine overwrites/hides: undraw their `.wnd` placeholders and defaults.
    let hidden: Vec<String> = (0..8)
        .flat_map(|i| {
            [
                format!("StaticTextPlayer{i}"),
                format!("StaticTextSide{i}"),
                format!("StaticTextTeam{i}"),
                format!("ProgressLoad{i}"),
                format!("ButtonMapStartPosition{i}"),
            ]
        })
        .chain(["LocalGeneralPortrait".to_owned(), "LocalGeneralFeatures".to_owned(), "WinMapPreview".to_owned()])
        .collect();
    let hide = move |name: &str| hidden.iter().any(|s| name.ends_with(s.as_str()));
    if let Some(wnd) = &shell.load_wnd {
        // The `.wnd` gadgets and their fonts (`gadget.rs`); the replaced widgets are hidden above.
        let scene = crate::gadget::Scene {
            wnd,
            images: &shell.images,
            fonts: &shell.fonts,
            text: &shell.text,
            window: win,
            hide: &hide,
            reveal: &|_: &str| false,
            label: &|_: &str| None::<String>,
            checked: &|_: &str| None::<bool>,
            tint: &|_: &str| None::<bevy::prelude::Color>,
            fraction: &|_: &str| None::<f32>,
        };
        crate::gadget::render(&mut commands, root, &scene);
    }
    // Local general portrait / name / features (`LoadScreen.cpp:1278`): the `PlayerTemplate`'s
    // display name, `Features` label and the general persona's `BioPortraitLarge` (or the faction
    // logo). The portrait and features placeholders stay undrawn (in `hidden` above).
    let local_faction = config.slots.first().map_or(String::new(), |s| s.faction.clone());
    let entry = shell::faction_entry(&shell, &local_faction);
    let local_name = entry.map_or_else(
        || shell::faction_label_for(&shell, &local_faction),
        |e| e.general_name.clone(),
    );
    text_at_left(&mut commands, root, &shell, win, "LocalGeneralName", &local_name, Color::srgb(0.95, 0.92, 0.8));
    if let Some(entry) = entry {
        let features = entry.features.clone();
        text_at_left(&mut commands, root, &shell, win, "LocalGeneralFeatures", &features, Color::srgb(0.95, 0.95, 0.9));
        if let Some(rect) = shell.load_wnd.as_ref().and_then(|w| shell::wnd_rect(w, win, "LocalGeneralPortrait")) {
            if let Some(img) = shell.images.get(&entry.portrait) {
                commands.spawn((
                    Node { position_type: PositionType::Absolute, left: Val::Px(rect.0), top: Val::Px(rect.1), width: Val::Px(rect.2), height: Val::Px(rect.3), ..default() },
                    ImageNode { image: img.handle.clone(), rect: img.rect, image_mode: NodeImageMode::Stretch, ..default() },
                    ChildOf(root),
                ));
            }
        }
    }
    // Occupied slots fill the first `netSlot` widgets (`MultiPlayerLoadScreen::init`).
    for (net, slot) in config.slots.iter().enumerate() {
        let rgb = shell::color_rgb(&shell, slot.color, fallback_color(net));
        let color = Color::srgb(rgb[0] as f32 / 255.0, rgb[1] as f32 / 255.0, rgb[2] as f32 / 255.0);
        // The player column: the human's name, or the AI's army name by difficulty
        // (`MultiPlayerLoadScreen` shows `Medium Army` for a Normal AI, as the setup list does).
        let name = if slot.is_ai { shell::ai_army_label(&shell.text, slot.difficulty) } else { shell::human_name() };
        let side = shell::faction_label_for(&shell, &slot.faction);
        // `teamStr.format("Team:%d", slot->getTeamNumber() + 1)`.
        let team = shell.text.get(&format!("Team:{}", slot.team + 1)).map(str::to_owned).unwrap_or_default();
        text_at_left(&mut commands, root, &shell, win, &format!("StaticTextPlayer{net}"), &name, color);
        text_at_left(&mut commands, root, &shell, win, &format!("StaticTextSide{net}"), &side, color);
        text_at(&mut commands, root, &shell, win, &format!("StaticTextTeam{net}"), &team, color);
        // `if (slot->isAI() && m_progressBars[netSlot]) m_progressBars[netSlot]->winHide(TRUE)`.
        if !slot.is_ai {
            progress_bar(&mut commands, root, &shell, win, net, slot.color, color);
        }
    }
    // Map preview in `WinMapPreview`: the map `.tga` (`getMapPreviewImage`), or the window's black
    // draw-data fill when the map has none (`winClearStatus(WIN_STATUS_IMAGE)`).
    if let Some(rect) = shell.load_wnd.as_ref().and_then(|w| shell::wnd_rect(w, win, "WinMapPreview")) {
        let mut e = commands.spawn((
            Node { position_type: PositionType::Absolute, left: Val::Px(rect.0), top: Val::Px(rect.1), width: Val::Px(rect.2), height: Val::Px(rect.3), ..default() },
            BackgroundColor(Color::srgb(0.0, 0.0, 0.0)),
            ChildOf(root),
        ));
        if let Some(preview) = find_map(&shell, &config.map).and_then(|m| m.preview.clone()) {
            e.insert(ImageNode { image: preview, image_mode: NodeImageMode::Stretch, ..default() });
        }
    }
    // Start-position markers (`positionStartSpots` / `updateMapStartSpots`,
    // `SkirmishGameOptionsMenu.cpp:674/786`): a `PlayerStart` button at each occupied start's
    // preview position, numbered `NUMBER:<slot+1>`.
    spawn_start_markers(&mut commands, root, &shell, win, &config);
}

/// The `PlayerStart` markers over `WinMapPreview` (`positionStartSpotControls`,
/// `SkirmishGameOptionsMenu.cpp:559` + `findDrawPositions`, `MapUtil.cpp:1317`).
fn spawn_start_markers(commands: &mut Commands, root: Entity, shell: &ShellData, win: Option<&Window>, config: &shell::GameConfig) {
    let Some(wnd) = &shell.load_wnd else { return };
    let (Some(preview), Some(map)) = (shell::wnd_rect(wnd, win, "WinMapPreview"), find_map(shell, &config.map)) else { return };
    let marker = shell::wnd_rect(wnd, win, "ButtonMapStartPosition0");
    let (mw, mh) = marker.map(|r| (r.2, r.3)).unwrap_or((17.0, 17.0));
    let image = shell.images.get("PlayerStart");
    let mut placed: Vec<(f32, f32)> = Vec::new();
    // `positionStartSpots` shows a marker at every `Player_N_Start` (`i < m_numPlayers`);
    // `updateMapStartSpots` only numbers the spots an occupied slot chose (`NUMBER:<slot+1>`).
    for (spot, &start) in map.starts.iter().enumerate() {
        let (mut gx, mut gy) = start_spot(map, preview, start, (mw, mh));
        // `positionStartSpotControls`' overlap nudge against earlier markers.
        for (px, py) in &placed {
            if gx > *px && gx < *px + mw && gy > *py && gy < *py + mh {
                let closer_right = px + mw - gx;
                let closer_bottom = py + mh - gy;
                if closer_right < closer_bottom {
                    gx = px + mw + 1.0;
                } else if closer_bottom < closer_right {
                    gy = py + mh + 1.0;
                } else {
                    gx = px + mw + 1.0;
                    gy = py + mh + 1.0;
                }
            }
        }
        placed.push((gx, gy));
        let mut e = commands.spawn((
            Node { position_type: PositionType::Absolute, left: Val::Px(gx), top: Val::Px(gy), width: Val::Px(mw), height: Val::Px(mh), ..default() },
            ChildOf(root),
        ));
        if let Some(img) = image {
            e.insert(ImageNode { image: img.handle.clone(), rect: img.rect, image_mode: NodeImageMode::Stretch, ..default() });
        }
        let label = config
            .slots
            .iter()
            .position(|s| s.start_position as usize == spot)
            .and_then(|slot| shell.text.get(&format!("NUMBER:{}", slot + 1)).map(str::to_owned))
            .unwrap_or_default();
        if !label.is_empty() {
            commands.spawn((
                Text::new(label),
                TextFont { font: crate::gadget::ui_font(&shell.fonts, "arial"), font_size: FontSize::Px((mh * 0.62).clamp(8.0, 16.0)), ..default() },
                TextColor(Color::srgb(0.996, 0.996, 0.996)),
                TextLayout { justify: Justify::Center, ..default() },
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(gx),
                    top: Val::Px(gy),
                    width: Val::Px(mw),
                    height: Val::Px(mh),
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                ChildOf(root),
            ));
        }
    }
}

/// `findDrawPositions` (`MapUtil.cpp:1317`): the map extent drawn aspect-preserved inside the
/// preview window, then the start's relative position (Y flipped) minus half the marker size.
fn start_spot(map: &shell::MapEntry, preview: (f32, f32, f32, f32), pos: [f32; 2], size: (f32, f32)) -> (f32, f32) {
    let (px, py, pw, ph) = preview;
    let ew = (map.extent_max[0] - map.extent_min[0]).max(1.0);
    let eh = (map.extent_max[1] - map.extent_min[1]).max(1.0);
    let ratio_width = ew / pw.max(1.0);
    let ratio_height = eh / ph.max(1.0);
    let (ulx, uly, sw, sh) = if ratio_width >= ratio_height {
        (0.0, (ph - eh / ratio_width) * 0.5, ew / ratio_width, eh / ratio_width)
    } else {
        ((pw - ew / ratio_height) * 0.5, 0.0, ew / ratio_height, eh / ratio_height)
    };
    let fx = (pos[0] - map.extent_min[0]) / ew;
    let fy = (pos[1] - map.extent_min[1]) / eh;
    (px + ulx + fx * sw - size.0 * 0.5, py + uly + (1.0 - fy) * sh - size.1 * 0.5)
}

/// Resolves the `MapCache` entry for a `GameConfig::map`, which is either a full VFS path (the menu
/// path, `to_game`) or a bare map name / `--quick` default (`load_map`'s path rule).
fn find_map<'a>(shell: &'a ShellData, map: &str) -> Option<&'a shell::MapEntry> {
    let want = if map.to_ascii_lowercase().ends_with(".map") {
        map.to_owned()
    } else {
        format!(r"maps\{map}\{map}.map")
    };
    shell.maps.iter().find(|m| m.path.eq_ignore_ascii_case(&want)).or_else(|| {
        shell.maps.iter().find(|m| {
            let base = m.path.rsplit(['\\', '/']).next().unwrap_or(&m.path).trim_end_matches(".map");
            base.eq_ignore_ascii_case(map)
        })
    })
}

/// A `ProgressLoadN` bar drawn from the window's draw data images, not a solid fill: the
/// `LoadingBar_DePowered` trough, and a house-coloured `LoadingBar_ProgressCenterN` fill clipped to
/// the load percentage (`MultiPlayerLoadScreen::init` sets draw-data state 6 to
/// `winSetEnabledImage(6, LoadingBar_ProgressCenter%d)`, falling back to `LoadingBar_Progress`;
/// `W3DGadgetProgressBarImageDraw` repeats that centre over the filled width).
///
/// The bar images are 3x11 atlas tiles, so they are drawn with `NodeImageMode::Stretch` (the default
/// `Auto` preserves the tile's aspect ratio and shrinks the bar to its row height).
fn progress_bar(commands: &mut Commands, root: Entity, shell: &ShellData, win: Option<&Window>, index: usize, slot_color: i32, color: Color) {
    let Some(wnd) = &shell.load_wnd else { return };
    let Some(rect) = shell::wnd_rect(wnd, win, &format!("ProgressLoad{index}")) else { return };
    let (w, h) = (rect.2, rect.3);
    let bar = commands
        .spawn((
            Node { position_type: PositionType::Absolute, left: Val::Px(rect.0), top: Val::Px(rect.1), width: Val::Px(w), height: Val::Px(h), overflow: Overflow::clip(), ..default() },
            ChildOf(root),
        ))
        .id();
    if let Some(entry) = shell.images.get("LoadingBar_DePowered") {
        commands.spawn((
            Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Px(w), height: Val::Px(h), ..default() },
            ImageNode { image: entry.handle.clone(), rect: entry.rect, image_mode: NodeImageMode::Stretch, ..default() },
            ChildOf(bar),
        ));
    }
    // The clipped fill: the tinted centre image is drawn at the full bar width inside a
    // progress%-wide node, so the visible part is the filled share of the bar.
    let fill = commands
        .spawn((
            LoadBarFill,
            Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Percent(0.0), height: Val::Px(h), overflow: Overflow::clip(), ..default() },
            ChildOf(bar),
        ))
        .id();
    // The house-coloured centre image (`LoadingBar_ProgressCenterN`), else the generic one.
    let center = format!("LoadingBar_ProgressCenter{slot_color}");
    let fill_img = shell
        .images
        .get(&center)
        .or_else(|| shell.images.get("LoadingBar_Progress"));
    if let Some(entry) = fill_img {
        commands.spawn((
            Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Px(w), height: Val::Px(h), ..default() },
            ImageNode { image: entry.handle.clone(), rect: entry.rect, image_mode: NodeImageMode::Stretch, color, ..default() },
            ChildOf(fill),
        ));
    }
}

/// A team-less slot falls back to a small palette; the setup screen keeps `color = -1` until chosen.
fn fallback_color(index: usize) -> [u8; 3] {
    const PALETTE: [[u8; 3]; 8] = [
        [255, 220, 0],
        [0, 180, 255],
        [235, 60, 60],
        [90, 220, 90],
        [200, 120, 255],
        [255, 160, 40],
        [120, 255, 220],
        [255, 100, 200],
    ];
    PALETTE[index.min(PALETTE.len() - 1)]
}

/// A centred label in the `.wnd` window `name`.
fn text_at(commands: &mut Commands, root: Entity, shell: &ShellData, win: Option<&Window>, name: &str, text: &str, color: Color) {
    text_in(commands, root, shell, win, name, text, color, Justify::Center);
}

/// A left-aligned label in the `.wnd` window `name` (the player columns and the general's text,
/// which `MultiplayerLoadScreen.wnd` draws left-aligned).
fn text_at_left(commands: &mut Commands, root: Entity, shell: &ShellData, win: Option<&Window>, name: &str, text: &str, color: Color) {
    text_in(commands, root, shell, win, name, text, color, Justify::Left);
}

#[allow(clippy::too_many_arguments)]
fn text_in(commands: &mut Commands, root: Entity, shell: &ShellData, win: Option<&Window>, name: &str, text: &str, color: Color, justify: Justify) {
    let Some(wnd) = &shell.load_wnd else { return };
    let Some(rect) = shell::wnd_rect(wnd, win, name) else { return };
    let size = (rect.3 * 0.6).clamp(10.0, 18.0);
    let left_aligned = justify == Justify::Left;
    commands.spawn((
        Text::new(text.to_owned()),
        TextFont { font: crate::gadget::ui_font(&shell.fonts, "arial"), font_size: FontSize::Px(size), ..default() },
        TextColor(color),
        TextLayout { justify, ..default() },
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(rect.0),
            top: Val::Px(rect.1),
            width: Val::Px(rect.2),
            height: Val::Px(rect.3),
            align_items: AlignItems::Center,
            justify_content: if left_aligned { JustifyContent::FlexStart } else { JustifyContent::Center },
            ..default()
        },
        ChildOf(root),
    ));
}

/// `--load-screen`: hold the bar for the screenshot harness (the app exits on its own). Otherwise
/// the `build` stages drive [`LoadProgress`].
fn loading_hold(mut wait: ResMut<LoadWait>, mut progress: ResMut<LoadProgress>) {
    if std::env::args().any(|a| a == "--load-screen") {
        wait.0 = wait.0.saturating_add(1);
        progress.0 = 40;
    }
}

fn load_bar_refresh(progress: Res<LoadProgress>, mut fills: Query<&mut Node, With<LoadBarFill>>) {
    for mut node in &mut fills {
        node.width = Val::Percent(progress.0.clamp(0, 100) as f32);
    }
}

fn stop_loading(mut commands: Commands, roots: Query<Entity, With<LoadScreenRoot>>) {
    for e in &roots {
        commands.entity(e).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `findDrawPositions` + `positionStartSpotControls` for Alpine Assault's `Player_1_Start`
    /// (extent 2400x2600, preview 428x300 at 1058,180, marker 34x25.5).
    #[test]
    fn start_spot_matches_find_draw_positions() {
        let map = shell::MapEntry {
            path: r"maps\alpine assault\alpine assault.map".into(),
            display: "Alpine Assault".into(),
            num_players: 2,
            multiplayer: true,
            preview: None,
            extent_min: [0.0, 0.0],
            extent_max: [2400.0, 2600.0],
            starts: vec![[1362.71, 1966.35], [1063.47, 428.23]],
        };
        let (x, y) = start_spot(&map, (1058.0, 180.0, 428.0, 300.0), map.starts[0], (34.0, 25.5));
        assert!((x - 1273.76).abs() < 0.1, "x = {x}");
        assert!((y - 240.36).abs() < 0.1, "y = {y}");
    }
}
