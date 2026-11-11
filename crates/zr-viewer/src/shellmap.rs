//! Shell map (`GameData::m_shellMapName`) rendered behind the shell menus.
//!
//! The engine starts the shell map as `GAME_SHELL` when the shell turns it on
//! (`Shell::showShellMap`, `Core/GameEngine/Source/GameClient/GUI/Shell/Shell.cpp:547`): it posts
//! `MSG_NEW_GAME` with `GAME_SHELL` and `TheWritableGlobalData->m_pendingFile =
//! TheGlobalData->m_shellMapName`. `GameLogic::startNewGame` loads it like any map, but with the
//! map's own sides (no skirmish slots) and the shell load screen; when the load finishes,
//! `GameLogic` pushes/hides `Menus/MainMenu.wnd` instead of the control bar (`GameLogic.cpp:2267`).
//!
//! The map runs as a live sim: `zr_game::Game::new_shell` gives every named side its player, script
//! list, teams and relations (`PlayerList::newGame`), with no skirmish slots, AI or victory check.
//! Its logic objects are drawn by the game's own `sim_step` / `sim_sync` (through this module's
//! wrappers, in the shell states), and the terrain props stay static. The camera follows the map's
//! camera script (`MOVE_CAMERA_TO` / `CAMERA_MOD_LOOK_TOWARD`). Script actions the sim does not
//! implement yet are reported by the sim's warning counters (`ScriptEngine::warnings`).
//!
//! The scene lives while the shell screens are up (`MainMenu`/`Setup`) and is torn down when a game
//! starts, so `build`'s staged world build owns the entities from `Loading` on.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use zr_assets::Vfs;
use zr_game::{Game, GameData, PlayerKind};
use zr_ini::IniDb;
use zr_map::MapFile;
use zr_sim::ObjectId;

use crate::anim;
use crate::convert;
use crate::lighting;
use crate::shell::ShellState;
use crate::sim_view::{self, ModelCache, SimObject, SimRes, Snap};
use crate::source::{self, Ground};
use crate::team_color::{self, TeamColors};

/// Marker for every entity the shell scene spawns (torn down when a game starts).
#[derive(Component)]
pub struct ShellMapEntity;

/// The shell map's live sim, handed from `build_scene` to [`install_shell_sim`] (the sim holds
/// `dyn` modules, so it is a non-send resource).
#[derive(Default)]
pub struct PendingShellSim(Option<SimRes>);

/// The shell scene's 3D camera.
#[derive(Component)]
pub struct ShellMapCamera;

/// The map's camera script, reduced to key points. `points[i] -> points[i+1]` takes
/// `seconds[i]`, looking at `look[i]`.
#[derive(Resource)]
pub struct ShellMapScene {
    height: zr_map::HeightMap,
    points: Vec<[f32; 2]>,
    seconds: Vec<f32>,
    look: Vec<[f32; 2]>,
    elapsed: f32,
    total: f32,
    /// Fallback: a fixed point with a slow orbit when the map has no camera script.
    spin: bool,
    dist: f32,
}

/// Camera pitch (`TheTacticalView->setPitchToDefault`, the viewer's RTS default).
const SHELL_PITCH: f32 = crate::camera::RTS_PITCH;

/// `GameData::ShellMapName` / `ShellMapOn` (`GlobalData.cpp` defaults, overridden by `GameData.ini`).
fn shell_map_settings(ini: &IniDb) -> (String, bool) {
    let mut name = "Maps\\ShellMap1\\ShellMap1.map".to_owned();
    let mut on = true;
    if let Some(b) = ini.of_kind("GameData").next() {
        if let Some(v) = b.value("ShellMapName").map(|s| s.trim()).filter(|s| !s.is_empty()) {
            name = v.to_owned();
        }
        if let Some(v) = b.value("ShellMapOn") {
            on = matches!(v.trim().to_ascii_lowercase().as_str(), "yes" | "true" | "1");
        }
    }
    (name, on)
}

/// `TheShell->showShellMap(TRUE)` at menu entry, but only when a game is not being started directly.
pub fn add_systems(app: &mut App) {
    // Built from `Update` (not `OnEnter`) so a return to `Setup` after a game rebuilds it cleanly:
    // `enter_setup`'s teardown runs first, then the next frame rebuilds the scene.
    let shell = in_state(ShellState::MainMenu).or_else(in_state(ShellState::Setup));
    app.insert_non_send(PendingShellSim::default());
    app.add_systems(Update, build_scene.run_if(shell.clone()).run_if(scene_absent));
    app.add_systems(Update, install_shell_sim.after(build_scene));
    app.add_systems(Update, tag_ground.run_if(resource_exists::<ShellMapScene>));
    // The shell map's own sim: the same step and sync as a game (`sim_view`), in the shell states.
    // Own wrappers: `sim_view::sim_step` / `sim_sync` already run in the game's schedule, and other
    // systems order against their type sets, which would be ambiguous with a second instance.
    app.add_systems(
        Update,
        (shell_sim_step, shell_sim_sync)
            .chain()
            .after(install_shell_sim)
            .run_if(shell.clone())
            .run_if(resource_exists::<ModelCache>)
            .run_if(resource_exists::<ShellSimActive>),
    );
    app.add_systems(Update, update_camera.run_if(shell));
}

/// `sim_view::sim_step` for the shell sim (the same body; see [`add_systems`]).
fn shell_sim_step(
    time: Res<Time>,
    sim: NonSendMut<SimRes>,
    fx_queue: ResMut<crate::fx::FxQueue>,
    paused: Res<crate::Paused>,
    replay: Option<ResMut<sim_view::ReplayPlayback>>,
    settings: Option<Res<crate::options::GameSettings>>,
) {
    sim_view::sim_step(time, sim, fx_queue, paused, replay, settings);
}

/// `sim_view::sim_sync` for the shell sim (the same body; see [`add_systems`]).
#[allow(clippy::too_many_arguments)]
fn shell_sim_sync(
    commands: Commands,
    sim: NonSend<SimRes>,
    cache: ResMut<ModelCache>,
    anim_cache: ResMut<anim::AnimCache>,
    module_cache: ResMut<crate::weapon_modules::WeaponModuleCache>,
    inv_bind: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    meshes: ResMut<Assets<Mesh>>,
    materials: ResMut<Assets<StandardMaterial>>,
    images: ResMut<Assets<Image>>,
    objs: Query<(Entity, &SimObject, &mut sim_view::SimModel, &mut Transform, Option<&crate::shroud::GhostSnapshot>)>,
) {
    sim_view::sim_sync(commands, sim, cache, anim_cache, module_cache, inv_bind, meshes, materials, images, objs);
}

/// Send-able marker that the shell map's sim is installed. Run conditions must not read `NonSend`
/// data: the executor can evaluate them on a worker thread, which aborts ("Attempted to access or
/// drop non-send resource"). Same pattern as `sim_view::SimActive`.
#[derive(Resource)]
struct ShellSimActive;

/// Moves the built shell sim into its non-send resource (exclusive, so the `!Send` sim can move).
fn install_shell_sim(world: &mut World) {
    let Some(sim) = world.non_send_mut::<PendingShellSim>().0.take() else { return };
    world.insert_non_send(sim);
    world.insert_resource(ShellSimActive);
}

/// A new interpolation snapshot of every live object (`sim_view::snapshot`).
fn snapshot(game: &Game) -> BTreeMap<ObjectId, Snap> {
    game.objects().map(|o| (o.id, Snap { pos: [o.position.x, o.position.y, o.position.z], angle: o.angle })).collect()
}

/// The water, road and sky entities that do not carry [`ShellMapEntity`] yet.
type UntaggedGround = (Or<(With<crate::water::WaterEntity>, With<crate::roads::RoadEntity>, With<crate::sky::SkyBoxRoot>)>, Without<ShellMapEntity>);

/// Tags the water, road and sky entities the shell's ground build spawned (`roads::spawn`,
/// `water::spawn`, `sky::spawn`) with [`ShellMapEntity`], so [`teardown`] owns them with the rest of the
/// shell scene. They already carry their module's own teardown tag (`WaterEntity`, `RoadEntity`,
/// `SkyBoxRoot`), which removes them when a game starts too.
fn tag_ground(mut commands: Commands, untagged: Query<Entity, UntaggedGround>) {
    for e in &untagged {
        commands.entity(e).insert(ShellMapEntity);
    }
}

fn scene_absent(scene: Option<Res<ShellMapScene>>) -> bool {
    scene.is_none()
}

/// Loads the shell map and spawns its world once (terrain, ground, static objects, camera).
#[allow(clippy::too_many_arguments)]
fn build_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut terrain_materials: ResMut<Assets<crate::terrain::TerrainMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut inv_bind: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut pending: NonSendMut<PendingShellSim>,
    settings: Option<Res<crate::options::GameSettings>>,
) {
    let Some(install) = zr_assets::Installs::detect().zero_hour else {
        return;
    };
    let Ok(vfs) = Vfs::zero_hour(&install) else { return };
    let ini = zr_ini::load_zero_hour(&vfs).db;
    // The shell map parses the INI itself, so it is where the launch `TextureReduction` is resolved for the
    // menu's model textures too (`GameLOD.cpp:565-580`).
    if let Some(s) = settings.as_deref() {
        crate::options::apply_texture_reduction(s, &crate::options::LodPresets::from_ini(&ini));
    }
    let (name, on) = shell_map_settings(&ini);
    if !on {
        return;
    }
    let path = name.to_ascii_lowercase();
    let path = if path.ends_with(".map") { path } else { format!(r"maps\{path}\{path}.map") };
    let Some(raw) = vfs.read(&path) else {
        warn!("shellmap: {path} not found");
        return;
    };
    let Ok(bytes) = zr_assets::compression::decompress_if_needed(&raw) else { return };
    let Ok(map) = MapFile::parse(&bytes) else { return };
    let Some(hm) = map.height_map.clone() else { return };
    let ground = Ground::new(&hm);
    let light = lighting::map_lighting(&map);
    let t0 = std::time::Instant::now();
    // The shell map as a `GAME_SHELL` sim: its sides, scripts and teams run; logic objects are
    // drawn from the sim (`sim_sync`), the rest stay static.
    let game = match Game::new_shell(Arc::new(GameData::load(&vfs)), &vfs, &path, 1) {
        Ok(game) => Some(game),
        Err(e) => {
            warn!("shellmap: sim not started for {path}: {e:#}");
            None
        }
    };

    let mut scene = ShellMapScene {
        height: hm.clone(),
        points: Vec::new(),
        seconds: Vec::new(),
        look: Vec::new(),
        elapsed: 0.0,
        total: 0.0,
        spin: false,
        dist: 420.0,
    };
    build_camera_path(&map, &mut scene);

    // Terrain (`build`'s `Stage::Terrain`).
    if let Ok(mut t) = source::terrain(&map, &vfs, &ini) {
        lighting::bake_terrain(&mut t, &light, ground.playable_size());
        let shadow_color = lighting::shadow_color_linear(&light);
        let pieces = convert::terrain(&t, &mut images, shadow_color);
        commands.spawn((
            ShellMapEntity,
            Mesh3d(meshes.add(pieces.mesh)),
            MeshMaterial3d(terrain_materials.add(pieces.material)),
            Transform::IDENTITY,
            bevy::light::NotShadowCaster,
        ));
        // The 3-way blend pass (`HeightMap.cpp` `renderExtraBlendTiles`).
        if let Some((mesh, material)) = pieces.extra {
            commands.spawn((
                ShellMapEntity,
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(terrain_materials.add(material)),
                Transform::IDENTITY,
                bevy::light::NotShadowCaster,
            ));
        }
    }

    // Ground: roads, water, sky, shadows (`build`'s `Stage::Ground`).
    crate::roads::spawn(&mut commands, &mut meshes, &mut materials, &mut terrain_materials, &mut images, &map, &vfs, &ini, &ground, &light);
    crate::water::spawn(&mut commands, &mut meshes, &mut materials, &mut images, &map, &vfs, &ini, &ground, &light);
    {
        let (pw, ph) = ground.playable_size();
        let settings = crate::sky::SkySettings::from_ini(&ini);
        crate::sky::spawn(&mut commands, &mut meshes, &mut materials, &mut images, &vfs, settings, (pw * pw + ph * ph).sqrt() * 0.5);
    }
    let tod = light.tod_index();
    let sun = convert::to_bevy(light.objects[tod][0].direction).normalize_or_zero();
    commands.insert_resource(crate::shadow::spawn_assets(&mut meshes, sun, light.shadow_color));

    // Static map objects (`build`'s `Stage::Objects`): every visible object is drawn, since there is
    // no sim to own the logic objects.
    spawn_objects(&mut commands, &mut meshes, &mut materials, &mut images, &mut inv_bind, &map, &vfs, &ini, &ground, game.as_ref());
    lighting::spawn_object_lights(&mut commands, &mut ambient, &light);

    // The shell camera (`GameLogic` looks at `InitialCameraPosition`, then `initHeightForMap` /
    // `setAngleToDefault` / `setPitchToDefault`; the scripts move it from there).
    let focus = scene.points.first().copied().unwrap_or([0.0, 0.0]);
    let dist = scene.dist;
    let pos = Vec3::new(focus[0], ground.height(focus[0], focus[1]), -focus[1]);
    let cam = crate::camera::rts_transform(&crate::camera::RtsCam {
        focus: pos,
        yaw: 0.0,
        pitch: SHELL_PITCH,
        dist,
        min_dist: 50.0,
        max_dist: 4000.0,
    });
    commands.spawn((
        ShellMapEntity,
        ShellMapCamera,
        Camera3d::default(),
        // Below the shell UI camera, so the menu draws over the world.
        Camera { order: -1, ..default() },
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        Projection::Perspective(PerspectiveProjection { far: 20000.0, near: 5.0, ..default() }),
        cam,
        bevy::audio::SpatialListener::new(4.0),
    ));
    if let Some(game) = game {
        // Player colours as `install` does: house colours from the map's sides, neutral for `Neutral`.
        let colors = game
            .players()
            .iter()
            .map(|p| (p.index, if p.kind == PlayerKind::Neutral { team_color::NEUTRAL } else { TeamColors::named_player(&map, &ini, &p.name) }))
            .collect();
        let snap = snapshot(&game);
        pending.0 = Some(SimRes {
            game,
            acc: 0.0,
            prev: snap.clone(),
            cur: snap,
            local: 0,
            selected: Vec::new(),
            #[cfg(feature = "possession")]
            possess: Default::default(),
        });
        commands.insert_resource(ModelCache::new(ini, vfs, colors));
        commands.insert_resource(anim::AnimCache::default());
    }
    let points = scene.points.len();
    commands.insert_resource(scene);
    info!(
        "shellmap: {path} built in {:?}: {} camera points, {} objects, {}x{} cells",
        t0.elapsed(),
        points,
        map.objects.len(),
        hm.width,
        hm.height
    );
}

/// The visible static map objects, with the same model / house-colour handling as `build`.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn spawn_objects(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    inv_bind: &mut Assets<SkinnedMeshInverseBindposes>,
    map: &MapFile,
    vfs: &Vfs,
    ini: &IniDb,
    ground: &Ground,
    game: Option<&Game>,
) {
    let teams = crate::team_color::TeamColors::new(map, ini);
    let mut raw: HashMap<String, Option<Vec<source::ModelPart>>> = HashMap::new();
    let mut parts: HashMap<(String, [u8; 3]), Vec<(Handle<Mesh>, Handle<StandardMaterial>)>> = HashMap::new();
    let mut anim_cache = anim::AnimCache::default();
    let mut spawned = 0usize;
    // Logic objects belong to the shell sim (`sim_sync`); only the terrain props stay static.
    for o in map.objects.iter().filter(|o| source::is_visible(o) && !game.is_some_and(|g| g.is_logic_map_object(o))) {
        let Some(src) = raw.entry(o.template.clone()).or_insert_with(|| source::object_model(ini, vfs, &o.template)) else { continue };
        let house = src.iter().any(crate::team_color::uses_house_color);
        let rgb = if house { teams.object_color(o) } else { crate::team_color::NEUTRAL };
        let asset = anim_cache.asset(ini, vfs, inv_bind, &o.template);
        let pivots = asset.as_ref().map(|a| a.hierarchy.pivot_count());
        let ps = parts.entry((o.template.clone(), rgb)).or_insert_with(|| {
            src.iter()
                .map(|p| {
                    let tinted;
                    let p = if house && crate::team_color::uses_house_color(p) {
                        tinted = crate::team_color::tint(p, rgb);
                        &tinted
                    } else {
                        p
                    };
                    let (m, mat) = convert::model_part(p, images, pivots);
                    (meshes.add(m), materials.add(mat))
                })
                .collect()
        });
        let mut pos = o.pos;
        pos[2] += ground.height(pos[0], pos[1]);
        let root = commands
            .spawn((ShellMapEntity, convert::object_transform(pos, o.angle), Visibility::default()))
            .id();
        anim::populate(commands, root, src.as_slice(), ps.as_slice(), asset.as_ref(), &zr_w3d::ConditionFlags::new(), &mut anim_cache, vfs);
        spawned += 1;
    }
    info!("shellmap: spawned {spawned} static objects");
}

/// Builds the camera key points from the map's `MOVE_CAMERA_TO` / `CAMERA_MOD_LOOK_TOWARD` actions.
///
/// `W3DView::moveCameraTo` moves the camera position along a waypoint path over `sec` seconds;
/// `cameraModLookToward` points the camera at the look-at waypoint (the engine's default camera is
/// rotated 90 degrees, so the heading is `atan2(dy, dx) - PI/2`, `W3DView.cpp:2926`). The path
/// starts at `InitialCameraPosition` (`GameLogic.cpp:2086`) and loops (the map's "Restart Camera
/// Script" re-enables the step scripts).
fn build_camera_path(map: &MapFile, scene: &mut ShellMapScene) {
    let waypoints: BTreeMap<String, [f32; 2]> = map
        .objects
        .iter()
        .filter_map(|o| o.props.str("waypointName").map(|n| (n.to_ascii_lowercase(), [o.pos[0], o.pos[1]])))
        .collect();
    let start = waypoints.get("initialcameraposition").copied().unwrap_or([0.0, 0.0]);
    scene.points.push(start);
    let Some(scripts) = map.scripts() else { scene.spin = true; scene.dist = 420.0; return };
    for sc in scripts.lists.iter().flat_map(|l| l.all_scripts()) {
        let Some(mv) = sc.actions.iter().find(|a| a.name() == Some("MOVE_CAMERA_TO")) else { continue };
        let Some(wp) = mv.params.first().and_then(|p| p.string()) else { continue };
        let Some(&pos) = waypoints.get(&wp.to_ascii_lowercase()) else { continue };
        let sec = mv.params.get(1).and_then(|p| p.real()).filter(|s| *s > 0.0).unwrap_or(10.0);
        let look = sc
            .actions
            .iter()
            .find(|a| a.name() == Some("CAMERA_MOD_LOOK_TOWARD"))
            .and_then(|a| a.params.first())
            .and_then(|p| p.string())
            .and_then(|n| waypoints.get(&n.to_ascii_lowercase()).copied())
            .unwrap_or(scene.look.last().copied().unwrap_or(start));
        scene.points.push(pos);
        scene.seconds.push(sec);
        scene.look.push(look);
    }
    if scene.seconds.is_empty() {
        scene.spin = true;
        scene.dist = 420.0;
        return;
    }
    scene.total = scene.seconds.iter().sum();
    // Frame the shell map's camera area: the waypoint spread gives a natural zoom.
    let (min, max) = scene.points.iter().fold(([f32::MAX; 2], [f32::MIN; 2]), |(mut mn, mut mx), p| {
        for i in 0..2 {
            mn[i] = mn[i].min(p[i]);
            mx[i] = mx[i].max(p[i]);
        }
        (mn, mx)
    });
    scene.dist = ((max[0] - min[0]).max(max[1] - min[1]) * 0.55).clamp(200.0, 900.0);
}

/// Advances the camera along the scripted path (`MOVE_CAMERA_TO` interpolation + look-toward yaw).
fn update_camera(
    time: Res<Time>,
    scene: Option<ResMut<ShellMapScene>>,
    mut cam: Query<&mut Transform, With<ShellMapCamera>>,
) {
    let Some(mut scene) = scene else { return };
    let Ok(mut t) = cam.single_mut() else { return };
    scene.elapsed += time.delta_secs();
    let ground = Ground::new(&scene.height);
    let (focus, yaw) = if scene.spin || scene.total <= 0.0 {
        let p = scene.points.first().copied().unwrap_or([0.0, 0.0]);
        (p, scene.elapsed * 0.15)
    } else {
        let mut local = scene.elapsed % scene.total;
        let mut seg = 0usize;
        while seg < scene.seconds.len() && local > scene.seconds[seg] {
            local -= scene.seconds[seg];
            seg += 1;
        }
        let seg = seg.min(scene.seconds.len() - 1);
        let a = scene.points[seg];
        let b = scene.points[seg + 1];
        let f = (local / scene.seconds[seg]).clamp(0.0, 1.0);
        let p = [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f];
        let look = scene.look[seg];
        let yaw = f32::atan2(look[1] - p[1], look[0] - p[0]) - std::f32::consts::FRAC_PI_2;
        (p, yaw)
    };
    let z = ground.height(focus[0], focus[1]);
    *t = crate::camera::rts_transform(&crate::camera::RtsCam {
        focus: Vec3::new(focus[0], z, -focus[1]),
        yaw,
        pitch: SHELL_PITCH,
        dist: scene.dist,
        min_dist: 50.0,
        max_dist: 4000.0,
    });
}

/// Despawns the shell scene (called before a game's staged build snapshots the entity set).
pub fn teardown(world: &mut World) {
    let mut q = world.query_filtered::<Entity, Or<(With<ShellMapEntity>, With<SimObject>)>>();
    let ids: Vec<Entity> = q.iter(world).collect();
    for e in ids {
        // A tagged child is removed with its tagged parent, so it may already be gone here.
        if world.get_entity(e).is_ok() {
            world.despawn(e);
        }
    }
    // The shell's sim and its model cache (the game's `build` installs its own).
    world.remove_non_send::<SimRes>();
    world.remove_resource::<ShellSimActive>();
    if let Some(mut pending) = world.get_non_send_mut::<PendingShellSim>() {
        pending.0 = None;
    }
    world.remove_resource::<ModelCache>();
    // The shell map's object lights (`lighting::spawn_object_lights`) are untagged directional
    // lights; left alive they stack with the game's own and brighten every object (the menu start
    // path rendered lighter than `--quick`). Nothing else spawns a directional light before the
    // game's `build` stage runs.
    let mut lights = world.query_filtered::<Entity, With<DirectionalLight>>();
    let ids: Vec<Entity> = lights.iter(world).collect();
    for e in ids {
        world.despawn(e);
    }
    world.remove_resource::<ShellMapScene>();
}
