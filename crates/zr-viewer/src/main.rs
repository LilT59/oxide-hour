//! Bevy viewer / playable skirmish shell for Zero Hour maps.
//!
//! Usage: `zr-viewer` opens the skirmish setup screen (`Menus/SkirmishGameOptionsMenu.wnd`).
//! Any command-line argument starts a game immediately (scripted / debug runs): `[map name]`
//! `[--quick] [--camera rts|overview] [--screenshot out.png] [--list] [--no-sim] [--sim-frames N]`
//! `[--auto-move] [--possess] [--script-input "W:60,D:30"] [--spawn <template>] [--fx <FXListName>]`
//! (`--possess`, `--first-person`, `--script-input` and `--radial-demo` need feature `possession`)
//! `[--duel] [--force-end win|lose] [--mute] [--menu]`. Debug HUD aids read straight from the
//! args: `[--select-first-structure] [--select-first-unit] [--hover-button N]`.
//!
//! Logic objects come from a live `zr-game` skirmish (left click selects, right click moves).

/// `set_changed!(place, value)`: writes only when the value differs. A `DerefMut` write marks the
/// component or asset changed, which re-runs UI layout, text measurement, visibility propagation or
/// a GPU re-upload, so an unchanged value must not be assigned (the picture is the same either way).
macro_rules! set_changed {
    ($place:expr, $value:expr) => {{
        let value = $value;
        if $place != value {
            $place = value;
        }
    }};
}

mod anim;
mod audio;
mod build;
mod camera;
mod chinook_view;
mod cmd_avail;
mod construct_modules;
mod construction_view;
mod context_cmd;
mod convert;
mod cursor;
mod eva;
mod fx;
mod hud;
mod lighting;
mod loading;
mod gadget;
mod mainmenu;
mod menu;
mod messages;
mod meta_keys;
mod options;
mod overlay;
mod perf;
mod pending_cursor;
mod physics_xform;
mod placement;
#[cfg(feature = "possession")]
mod possess;
mod projectile_draw;
mod promotions;
#[cfg(feature = "possession")]
mod radial;
mod radar;
mod rally_view;
mod roads;
mod rts_input;
mod scorch;
mod shadow;
mod shell;
mod shellmap;
mod shroud;
mod sim_view;
mod sky;
mod source;
mod superweapon_view;
mod team_color;
mod terrain;
mod terrain_shroud;
mod ui_block;
mod userdata;
mod vehicle_draw;
mod water;
mod weapon_modules;

use anyhow::{anyhow, Context};
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::window::WindowResolution;
use zr_assets::{compression::decompress_if_needed, Vfs};
use zr_map::MapFile;

use camera::{FreeCam, Interactive};
use shell::{PendingGame, ShellState};
use source::Ground;

#[derive(Resource)]
pub struct MapRes(pub MapFile);

impl MapRes {
    pub fn ground(&self) -> Ground<'_> {
        Ground::new(self.0.height_map.as_ref().expect("checked at load"))
    }
}

#[derive(Resource)]
pub struct Args {
    pub(crate) map: String,
    pub(crate) overview: bool,
    pub(crate) screenshot: Option<String>,
    /// Render frame at which `--screenshot` fires (default 90; raise for late-frame captures).
    pub(crate) screenshot_after: u32,
    pub(crate) list: bool,
    /// Silence all audio (`--mute`); screenshots mute automatically.
    pub(crate) mute: bool,
    /// Force the setup screen even when other arguments are present (`--menu`).
    pub(crate) menu: bool,
    /// Debug: start at the load screen with the default skirmish and hold it (`--load-screen`).
    pub(crate) load_screen: bool,
    /// Debug: end the game after ~20 logic frames (`--force-end win|lose`).
    pub(crate) force_end: Option<bool>,
    /// Debug: never run the victory check, so a long `--sim-frames` run keeps stepping after the
    /// local player is defeated (`--no-game-end`, perf runs).
    pub(crate) no_game_end: bool,
    /// Debug: click Start automatically after a short delay (`--auto-start`).
    pub(crate) auto_start: bool,
    /// Debug: quit to setup after a short delay (`--auto-quit`).
    pub(crate) auto_quit: bool,
    /// Debug: open the pause overlay after a short delay (`--auto-pause`).
    pub(crate) auto_pause: bool,
    /// Record this game's command stream to a replay file (`--record [name]`).
    pub(crate) record: Option<String>,
    /// Play back a replay file (`--replay <file>`), checking CRCs.
    pub(crate) replay: Option<String>,
    /// Debug: spawn one animated instance of this template at the map centre.
    pub(crate) spawn: Option<String>,
    /// Debug: play this `FXList` once at the camera focus.
    pub(crate) fx: Option<String>,
    /// Debug: place the RTS camera at a Generals-space `x,y` with this distance (`--camera x,y,dist`).
    pub(crate) camera: Option<(f32, f32, f32)>,
    pub(crate) sim: sim_view::SimArgs,
}

#[derive(Resource)]
struct ScreenshotState {
    frame: u32,
    taken_at: Option<u32>,
}

/// Freezes the simulation (pause menu / game over).
#[derive(Resource, Default)]
pub struct Paused(pub bool);

/// The in-game pause overlay is visible.
#[derive(Resource, Default)]
pub struct PauseMenu(pub bool);

/// The config of the running game (for Restart).
#[derive(Resource)]
pub struct CurrentGame(pub shell::GameConfig);

/// Result of the finished game, for the score screen.
#[derive(Resource, Default)]
pub struct GameResult {
    pub victory: bool,
    pub frame: u32,
    pub over: bool,
}

/// Entity IDs that existed before the current game build (engine internals + shell UI). Teardown
/// despawns everything else, which avoids `World::clear_entities` (it would nuke resources).
#[derive(Resource)]
struct GamePre(std::collections::HashSet<Entity>);

fn parse_args() -> Args {
    let mut a = Args {
        map: "alpine assault".into(),
        overview: false,
        screenshot: None,
        screenshot_after: 90,
        list: false,
        mute: false,
        menu: false,
        load_screen: false,
        force_end: None,
        no_game_end: false,
        auto_start: false,
        auto_quit: false,
        auto_pause: false,
        record: None,
        replay: None,
        spawn: None,
        fx: None,
        camera: None,
        sim: Default::default(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(s) = it.next() {
        match s.as_str() {
            "--screenshot" => a.screenshot = it.next(),
            "--shot-frame" => a.screenshot_after = it.next().and_then(|n| n.parse().ok()).unwrap_or(90),
            // `--camera overview` (wide map view) or `--camera x,y,dist` (focus in Generals space).
            "--camera" => match it.next().as_deref() {
                Some("overview") => a.overview = true,
                Some("rts") => a.overview = false,
                Some(v) => a.camera = parse_camera(v),
                None => {}
            },
            "--list" => a.list = true,
            "--mute" => a.mute = true,
            "--menu" => a.menu = true,
            "--load-screen" => a.load_screen = true,
            "--quick" => {}
            // Read from the process args by `meta_keys::dev_mode`; swallowed so it is not a map name.
            "--dev" => {}
            "--force-end" => a.force_end = Some(!matches!(it.next().as_deref(), Some("lose"))),
            "--no-game-end" => a.no_game_end = true,
            "--auto-start" => a.auto_start = true,
            "--auto-quit" => a.auto_quit = true,
            "--auto-pause" => a.auto_pause = true,
            "--record" => a.record = Some(it.next().unwrap_or_else(|| "replay.zrp".to_owned())),
            "--replay" => a.replay = it.next(),
            "--spawn" => a.spawn = it.next(),
            "--fx" => a.fx = it.next(),
            "--no-sim" => a.sim.disabled = true,
            "--sim-frames" => a.sim.prerun = it.next().and_then(|n| n.parse().ok()).unwrap_or(0),
            "--auto-move" => a.sim.auto_move = true,
            "--construct-demo" => a.sim.construct_demo = true,
            "--duel" => a.sim.duel = true,
            "--ai-view" => a.sim.focus_ai = true,
            "--ghost-view" => a.sim.ghost_view = true,
            "--chinook-demo" => a.sim.chinook_demo = true,
            "--superweapon-demo" => a.sim.superweapon_demo = true,
            // Read from the process args by `radial.rs`; swallowed here so it is not a map name.
            "--radial-demo" => {}
            // Debug HUD aid: hud.rs reads the flag from the process args.
            "--select-first-structure" | "--select-first-unit" => {}
            "--queue" | "--place" | "--place-pos" | "--special" | "--fire-power" => {
                let _ = it.next();
            }
            "--attack-move" | "--guard" | "--cooldown-demo" => {}
            #[cfg(feature = "possession")]
            "--possess" => a.sim.possess = true,
            "--possess-unit" => {
                #[cfg(feature = "possession")]
                {
                    a.sim.possess = true;
                }
                a.sim.possess_unit = it.next();
            }
            #[cfg(feature = "possession")]
            "--first-person" => {
                a.sim.possess = true;
                a.sim.first_person = true;
            }
            "--damage" => a.sim.damage = true,
            #[cfg(feature = "possession")]
            "--script-input" => a.sim.script = possess::parse_script(&it.next().unwrap_or_default()),
            // Possession flags in a build without it: ignored, but not taken as a map name.
            #[cfg(not(feature = "possession"))]
            "--possess" | "--first-person" | "--script-input" => {
                eprintln!("{s}: possession is not built in (cargo feature `possession`); ignored");
                if s == "--script-input" {
                    let _ = it.next();
                }
            }
            _ => a.map = s,
        }
    }
    a
}

/// `x,y,dist` for `--camera` (Generals-space focus and camera distance).
fn parse_camera(s: &str) -> Option<(f32, f32, f32)> {
    let mut it = s.split(',');
    let x = it.next()?.trim().parse().ok()?;
    let y = it.next()?.trim().parse().ok()?;
    let d = it.next()?.trim().parse().ok()?;
    Some((x, y, d))
}

fn main() {
    let args = parse_args();
    let record_path = args.record.clone();
    // Any argument starts a game immediately (scripted runs); no args shows the main menu.
    let immediate = !args.menu && !args.load_screen && std::env::args().count() > 1;
    // Immediate runs build the world on the load screen in `build` stages, then enter `InGame`.
    let initial = if args.load_screen || immediate {
        ShellState::Loading
    } else {
        ShellState::MainMenu
    };
    let interactive = args.screenshot.is_none();
    let mut app = App::new();
    app
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "zr-viewer".into(),
                resolution: WindowResolution::new(1600, 900),
                present_mode: perf::present_mode(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(terrain::TerrainMaterialPlugin)
        .insert_resource(ClearColor(Color::srgb(0.02, 0.025, 0.035)))
        .insert_resource(Interactive(interactive))
        .insert_resource(FreeCam::default())
        .insert_resource(camera::CamLock::default())
        .insert_resource(camera::ScrollDir::default())
        .init_resource::<camera::CameraDrag>()
        .insert_resource(args)
        .insert_resource(ScreenshotState { frame: 0, taken_at: None })
        .insert_resource(Paused(false))
        .insert_resource(PauseMenu(false))
        .insert_resource(GameResult::default())
        .insert_resource(RecordingPath(record_path))
        .insert_state(initial)
        .add_systems(Update, (screenshot_driver, frame_stats, auto_driver))
        .add_systems(Update, (camera::rts_camera, camera::free_camera).run_if(in_state(ShellState::InGame)).run_if(|m: ui_block::ModalOpen| !m.get()))
        .add_systems(Update, record_crcs.after(sim_view::sim_step).run_if(in_state(ShellState::InGame)))
        .add_systems(Update, replay_check.after(sim_view::sim_step).run_if(in_state(ShellState::InGame)))
        .add_systems(Last, save_replay_on_exit);
    if immediate || initial == ShellState::Loading {
        // The default Alpine skirmish (America human vs China AI) for `--quick` / screenshots.
        app.insert_resource(PendingGame(shell::GameConfig {
            map: "Alpine Assault".into(),
            slots: vec![zr_game::PlayerSetup::new("FactionAmerica", 0), zr_game::PlayerSetup::ai("FactionChina", 1)],
            starting_cash: 10000,
            superweapon_restriction: 0,
            seed: 1,
        }));
    }
    if !immediate {
        // The shell screens draw with a `Camera2d`; Setup and MainMenu are entered from each other.
        app.add_systems(OnEnter(ShellState::MainMenu), spawn_shell_camera);
        app.add_systems(OnExit(ShellState::MainMenu), despawn_shell_camera);
        app.add_systems(OnEnter(ShellState::Setup), (enter_setup, spawn_shell_camera).chain());
        app.add_systems(OnExit(ShellState::Setup), despawn_shell_camera);
        app.add_systems(OnEnter(ShellState::Loading), spawn_shell_camera);
        app.add_systems(OnExit(ShellState::Loading), despawn_shell_camera);
        // The shell map (`Shell::showShellMap`) lives behind the menu screens.
        shellmap::add_systems(&mut app);
    }
    // The shell data (menu images, pause / score / options `.wnd` files, the string table's layout)
    // loads on every path, including `--quick`, so the in-game screens draw the same as on the
    // menu path. A direct entry into `Loading` (`--load-screen`, `--quick`) has not been through
    // MainMenu/Setup; the menu path already has it (the run condition skips the reload).
    app.add_systems(OnEnter(ShellState::Loading), shell::setup_shell_data.run_if(|d: Option<Res<shell::ShellData>>| d.is_none()));
    // State transitions. The world is built in stages while on the load screen (`build`); this
    // snapshots the pre-game entities first so `teardown_game` can remove exactly the game's. The
    // shell scene is despawned first (exclusive) so its entities are not captured as "pre-game".
    app.add_systems(OnEnter(ShellState::Loading), shellmap::teardown.before(record_pre));
    // The shell map's roads, water and sky are spawned outside `ShellMapEntity` (see shellmap.rs).
    app.add_systems(OnEnter(ShellState::Loading), (roads::teardown, water::teardown, sky::teardown).before(record_pre));
    app.add_systems(OnEnter(ShellState::Loading), record_pre);
    app.add_systems(OnEnter(ShellState::Restart), restart_enter);
    // Esc first cancels a pending order (`placement::order_click`) or closes the options overlay
    // (`options::toggle_options`); it only opens the pause menu when neither consumed it.
    app.add_systems(
        Update,
        (pause_input.before(placement::order_click).before(options::toggle_options).before(promotions::close_on_escape), game_end_check)
            .run_if(in_state(ShellState::InGame)),
    );
    build::add_systems(&mut app);
    shell::add_systems(&mut app);
    menu::add_systems(&mut app);
    mainmenu::add_systems(&mut app);
    loading::add_systems(&mut app);
    sim_view::add_systems(&mut app);
    audio::add_systems(&mut app);
    anim::add_systems(&mut app);
    fx::add_systems(&mut app);
    scorch::add_systems(&mut app);
    chinook_view::add_systems(&mut app);
    superweapon_view::add_systems(&mut app);
    projectile_draw::add_systems(&mut app);
    #[cfg(feature = "possession")]
    radial::add_systems(&mut app);
    hud::add_systems(&mut app);
    promotions::add_systems(&mut app);
    options::add_systems(&mut app);
    radar::add_systems(&mut app);
    eva::add_systems(&mut app);
    messages::add_systems(&mut app);
    placement::add_systems(&mut app);
    rts_input::add_systems(&mut app);
    context_cmd::add_systems(&mut app);
    meta_keys::add_systems(&mut app);
    cursor::add_systems(&mut app);
    ui_block::add_systems(&mut app);
    #[cfg(feature = "possession")]
    possess::add_systems(&mut app);
    water::add_systems(&mut app);
    weapon_modules::add_systems(&mut app);
    rally_view::add_systems(&mut app);
    construct_modules::add_systems(&mut app);
    physics_xform::add_systems(&mut app);
    vehicle_draw::add_systems(&mut app);
    overlay::add_systems(&mut app);
    shroud::add_systems(&mut app);
    terrain_shroud::add_systems(&mut app);
    sky::add_systems(&mut app);
    shadow::add_systems(&mut app);
    perf::add_systems(&mut app);
    app.run();
}

pub(crate) fn load_map(vfs: &Vfs, name: &str) -> anyhow::Result<MapFile> {
    let name = name.to_ascii_lowercase();
    let path = if name.ends_with(".map") { name } else { format!(r"maps\{name}\{name}.map") };
    let raw = vfs.read(&path).ok_or_else(|| anyhow!("map not found in VFS: {path}"))?;
    let data = decompress_if_needed(&raw).context("decompressing map")?;
    MapFile::parse(&data).map_err(|e| anyhow!("parsing {path}: {e}"))
}

/// Loads a replay by name from the user data dir (or an absolute path).
pub(crate) fn load_replay(name: &str) -> anyhow::Result<zr_game::replay::Replay> {
    let path = if std::path::Path::new(name).is_absolute() {
        std::path::PathBuf::from(name)
    } else {
        crate::userdata::dir().join(name)
    };
    let bytes = std::fs::read(&path).with_context(|| format!("reading replay {}", path.display()))?;
    zr_game::replay::Replay::decode(&bytes).map_err(|e| anyhow!("{e}"))
}

/// Writes a recorded replay to the user data dir (or an absolute path).
fn save_replay(name: Option<String>, rec: &zr_game::replay::Replay) {
    let Some(name) = name else { return };
    let path = if std::path::Path::new(&name).is_absolute() {
        std::path::PathBuf::from(&name)
    } else {
        match crate::userdata::ensure() {
            Ok(d) => d.join(&name),
            Err(e) => {
                warn!("replay: {e}");
                return;
            }
        }
    };
    match std::fs::write(&path, rec.encode()) {
        Ok(()) => info!("replay: saved {} ({} events, {} CRCs) to {}", rec.header.map, rec.events.len(), rec.crcs.len(), path.display()),
        Err(e) => warn!("replay: could not write {}: {e}", path.display()),
    }
}

/// Where `--record` saves the replay (a bare name goes to the user data dir).
#[derive(Resource, Default)]
pub struct RecordingPath(pub Option<String>);

/// Removes every resource a game build inserts (kept so a restart/quit rebuilds cleanly).
pub fn remove_game_resources(world: &mut World) {
    if let Some(mut sim) = world.remove_non_send::<sim_view::SimRes>() {
        if let Some(rec) = sim.game.take_recording() {
            let name = world.get_resource::<RecordingPath>().and_then(|p| p.0.clone());
            save_replay(name, &rec);
        }
    }
    world.remove_resource::<sim_view::SimActive>();
    world.remove_resource::<MapRes>();
    world.remove_resource::<sim_view::ModelCache>();
    world.remove_resource::<hud::HudData>();
    world.remove_resource::<radar::Radar>();
    world.remove_resource::<audio::AudioBank>();
    world.remove_resource::<eva::EvaState>();
    world.remove_resource::<fx::FxDb>();
    world.remove_resource::<fx::FxAssets>();
    world.remove_resource::<fx::FxRng>();
    world.remove_resource::<camera::RtsCam>();
    world.remove_resource::<anim::AnimCache>();
    world.remove_resource::<shadow::ShadowAssets>();
    world.remove_resource::<scorch::Scorches>();
    world.remove_resource::<shroud::ShroudView>();
    world.remove_resource::<terrain_shroud::ShroudTintCache>();
    world.remove_resource::<CurrentGame>();
    world.remove_resource::<build::BuildJob>();
    if let Some(mut pending) = world.get_non_send_mut::<build::PendingSim>() {
        pending.0 = None;
    }
    if let Some(mut slot) = world.get_non_send_mut::<sim_view::SimSlot>() {
        slot.0 = None;
    }
    world.insert_resource(Paused(false));
    world.insert_resource(PauseMenu(false));
}

/// `OnEnter(Loading)`: snapshot the pre-game entity set so [`teardown_game`] can remove exactly the
/// game's entities (the staged `build` spawns them after this).
fn record_pre(world: &mut World) {
    let pre = world.iter_entities().map(|e| e.id()).collect();
    world.insert_resource(GamePre(pre));
}

/// Despawns every entity created since [`record_pre`] (excluding resources) and removes the game's
/// resources. A no-op when no game was recorded.
fn teardown_game(world: &mut World) {
    let Some(pre) = world.get_resource::<GamePre>().map(|g| g.0.clone()) else { return };
    remove_game_resources(world);
    let resource_entities: std::collections::HashSet<Entity> = world.resource_entities().iter().map(|(_, e)| e).collect();
    let to_despawn: Vec<Entity> = world
        .iter_entities()
        .map(|e| e.id())
        .filter(|e| !pre.contains(e) && !resource_entities.contains(e))
        .collect();
    for e in to_despawn {
        world.despawn(e);
    }
}

/// `OnEnter(Setup)`: clear any running game before the menu is built.
fn enter_setup(world: &mut World) {
    teardown_game(world);
}

/// `OnEnter(Restart)`: tear the game down, then rebuild from `PendingGame` on the load screen.
fn restart_enter(world: &mut World) {
    teardown_game(world);
    world.resource_mut::<NextState<ShellState>>().set(ShellState::Loading);
}

/// The shell renders `bevy_ui` on its own 2D camera (the game camera only exists in `InGame`).
#[derive(Component)]
struct ShellCamera;

fn spawn_shell_camera(mut commands: Commands) {
    // The shell map renders through a separate 3D camera; the UI camera draws on top (`order`) and
    // is marked as the default UI camera so `bevy_ui` does not pick the 3D one.
    commands.spawn((
        ShellCamera,
        Camera2d,
        Camera { order: 1, ..default() },
        bevy::ui::IsDefaultUiCamera,
        Transform::default(),
    ));
}

fn despawn_shell_camera(mut commands: Commands, q: Query<Entity, With<ShellCamera>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

/// Debug driver for scripted screenshots (`--auto-start` / `--auto-quit`).
#[allow(clippy::too_many_arguments)]
fn auto_driver(
    args: Res<Args>,
    state: Res<State<ShellState>>,
    shell: Option<Res<shell::ShellData>>,
    config: Option<Res<shell::ShellConfig>>,
    mut commands: Commands,
    mut next: ResMut<NextState<ShellState>>,
    mut paused: ResMut<Paused>,
    mut pause: ResMut<PauseMenu>,
    mut pause_dirty: ResMut<menu::PauseDirty>,
    mut frame: Local<u32>,
    existing: Query<Entity, With<menu::MenuRoot>>,
) {
    *frame += 1;
    match state.get() {
        ShellState::InGame if args.auto_pause && *frame == 60 => {
            pause.0 = true;
            paused.0 = true;
            pause_dirty.0 = true;
        }
        // `--menu --auto-start`: walk from the start screen into the skirmish setup.
        ShellState::MainMenu if args.auto_start && *frame > 20 => next.set(ShellState::Setup),
        // `--menu-setup`: stop on the skirmish setup screen (screenshot harness; no game starts).
        ShellState::MainMenu if std::env::args().any(|a| a == "--menu-setup") && *frame > 20 => next.set(ShellState::Setup),
        ShellState::Setup if args.auto_start && *frame > 20 => {
            if let (Some(data), Some(cfg)) = (&shell, &config) {
                let game = cfg.to_game(data);
                for e in &existing {
                    commands.entity(e).despawn();
                }
                commands.insert_resource(PendingGame(game));
                next.set(ShellState::Loading);
            }
        }
        ShellState::InGame if args.auto_quit && *frame > 60 => next.set(ShellState::Setup),
        _ => {}
    }
}

/// Esc toggles the in-game pause overlay.
fn pause_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut paused: ResMut<Paused>,
    mut pause: ResMut<PauseMenu>,
    mut dirty: ResMut<menu::PauseDirty>,
    mode: Res<placement::OrderMode>,
    options: Res<options::OptionsState>,
    promo: Res<promotions::PromotionsData>,
) {
    if keys.just_pressed(KeyCode::Escape) && mode.0.is_none() && !options.open && !promo.panel_open() {
        pause.0 = !pause.0;
        paused.0 = pause.0;
        dirty.0 = true;
    }
}

/// `VictoryConditions::checkForGameEnd` -> score screen.
fn game_end_check(
    sim: Option<NonSend<sim_view::SimRes>>,
    args: Res<Args>,
    mut paused: ResMut<Paused>,
    mut result: ResMut<GameResult>,
    mut next: ResMut<NextState<ShellState>>,
) {
    let Some(sim) = sim else { return };
    if result.over || args.no_game_end {
        return;
    }
    let local = sim.local;
    if let Some(win) = args.force_end {
        if sim.game.frame() >= 20 {
            result.victory = win;
            result.frame = sim.game.frame();
            result.over = true;
            paused.0 = true;
            next.set(ShellState::Score);
        }
        return;
    }
    let end = sim.game.logic().check_for_game_end();
    let defeated_local = end.defeated.contains(&local);
    if end.winner.is_some() || defeated_local {
        result.victory = end.winner == Some(local);
        result.frame = sim.game.frame();
        result.over = true;
        paused.0 = true;
        next.set(ShellState::Score);
    }
}

/// Saves the active recording when the app exits.
fn save_replay_on_exit(sim: Option<NonSendMut<sim_view::SimRes>>, path: Res<RecordingPath>, exits: MessageReader<AppExit>) {
    if exits.is_empty() {
        return;
    }
    if let Some(mut sim) = sim {
        if let Some(rec) = sim.game.take_recording() {
            save_replay(path.0.clone(), &rec);
        }
    }
}

/// `--replay`: checks the CRC checkpoint after stepping (`Recorder` playback desync report).
fn replay_check(sim: Option<NonSend<sim_view::SimRes>>, replay: Option<ResMut<sim_view::ReplayPlayback>>) {
    let (Some(sim), Some(mut pb)) = (sim, replay) else { return };
    pb.check(&sim);
}

/// `--record`: appends a state CRC every interval frames (`Recorder`'s periodic checkpoint).
fn record_crcs(sim: Option<NonSendMut<sim_view::SimRes>>) {
    let Some(mut sim) = sim else { return };
    if !sim.game.is_recording() {
        return;
    }
    let interval = sim.game.recording_crc_interval().max(1);
    if sim.game.frame() % interval == 0 {
        sim.game.record_crc();
    }
}

/// Logs the average render frame time every 120 frames (for the visual-fidelity report).
fn frame_stats(time: Res<Time>, mut frames: Local<u32>, mut acc: Local<f32>) {
    *acc += time.delta_secs();
    *frames += 1;
    if *frames >= 120 {
        info!("frame time: {:.2} ms ({:.0} fps)", *acc / *frames as f32 * 1000.0, *frames as f32 / *acc);
        *acc = 0.0;
        *frames = 0;
    }
}

fn screenshot_driver(mut commands: Commands, args: Res<Args>, mut st: ResMut<ScreenshotState>, mut exit: MessageWriter<AppExit>) {
    let Some(path) = &args.screenshot else { return };
    st.frame += 1;
    if st.taken_at.is_none() && st.frame >= args.screenshot_after {
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path.clone()));
        st.taken_at = Some(st.frame);
    }
    if let Some(t) = st.taken_at {
        if st.frame > t + 30 {
            exit.write(AppExit::Success);
        }
    }
}
