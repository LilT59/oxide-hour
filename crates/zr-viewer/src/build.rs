//! Staged skirmish world build, driven by the real `GameLogic::startNewGame` /
//! `updateLoadProgress` checkpoints.
//!
//! `GameLogic::startNewGame` (`GeneralsMD/.../GameLogic/System/GameLogic.cpp`) calls
//! `updateLoadProgress(LOAD_PROGRESS_*)` between coarse build steps, so the load screen bar sees
//! real progress rather than a timer. This module reproduces that by splitting the synchronous
//! build (formerly `main::try_setup`) into stages, one per rendered frame while
//! `ShellState::Loading`, advancing [`LoadProgress`] to the matching `LOAD_PROGRESS_*` value after
//! each stage. The work each stage performs is unchanged; only the frame boundary and the progress
//! call moved.
//!
//! The `LOAD_PROGRESS_*` enum below is copied from `GameLogic.cpp` (`MAX_SLOTS = 8`).

use std::sync::Arc;

use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use zr_assets::Vfs;
use zr_ini::IniDb;
use zr_map::{GlobalLighting, MapFile};

use crate::anim;
use crate::convert;
use crate::lighting;
use crate::loading::LoadProgress;
use crate::shell::{GameConfig, PendingGame, ShellData, ShellState};
use crate::sim_view::{SimArgs, SimInit, SimSlot};
use crate::source::{self, Ground};
use crate::terrain;
use crate::{Args, CurrentGame, MapRes};

// `GameLogic.cpp` load-screen progress checkpoints (`MAX_SLOTS = 8`).
#[allow(dead_code)]
const LOAD_PROGRESS_START: u32 = 0;
#[allow(dead_code)]
const LOAD_PROGRESS_POST_PARTICLE_INI_LOAD: u32 = 1;
const LOAD_PROGRESS_POST_LOAD_MAP: u32 = 2;
#[allow(dead_code)]
const LOAD_PROGRESS_SIDE_POPULATION: u32 = 3;
#[allow(dead_code)]
const LOAD_PROGRESS_POST_SIDE_LIST_INIT: u32 = LOAD_PROGRESS_SIDE_POPULATION + 1 + 8;
#[allow(dead_code)]
const LOAD_PROGRESS_POST_PLAYER_LIST_RESET: u32 = LOAD_PROGRESS_POST_SIDE_LIST_INIT + 1;
#[allow(dead_code)]
const LOAD_PROGRESS_POST_SCRIPT_ENGINE_NEW_MAP: u32 = LOAD_PROGRESS_POST_PLAYER_LIST_RESET + 1;
#[allow(dead_code)]
const LOAD_PROGRESS_POST_VICTORY_CONDITION_SETUP: u32 = LOAD_PROGRESS_POST_SCRIPT_ENGINE_NEW_MAP + 2;
#[allow(dead_code)]
const LOAD_PROGRESS_POST_VICTORY_CONDITION_SET_VICTORY_CONDITION: u32 = LOAD_PROGRESS_POST_VICTORY_CONDITION_SETUP + 1;
#[allow(dead_code)]
const LOAD_PROGRESS_POST_GHOST_OBJECT_MANAGER_RESET: u32 = LOAD_PROGRESS_POST_VICTORY_CONDITION_SET_VICTORY_CONDITION + 1;
const LOAD_PROGRESS_POST_TERRAIN_LOGIC_NEW_MAP: u32 = LOAD_PROGRESS_POST_GHOST_OBJECT_MANAGER_RESET + 1;
const LOAD_PROGRESS_POST_BRIDGE_LOAD: u32 = LOAD_PROGRESS_POST_TERRAIN_LOGIC_NEW_MAP + 1;
const LOAD_PROGRESS_POST_PATHFINDER_NEW_MAP: u32 = LOAD_PROGRESS_POST_BRIDGE_LOAD + 1;
const LOAD_PROGRESS_LOOP_ALL_THE_FREAKN_OBJECTS: u32 = LOAD_PROGRESS_POST_BRIDGE_LOAD + 1;
const LOAD_PROGRESS_MAX_ALL_THE_FREAKN_OBJECTS: u32 = 80;
/// A point inside the engine's 22..80 map-object loop, for the viewer's ground-geometry stage.
const LOAD_PROGRESS_OBJECT_LOOP_MID: u32 = (LOAD_PROGRESS_LOOP_ALL_THE_FREAKN_OBJECTS + LOAD_PROGRESS_MAX_ALL_THE_FREAKN_OBJECTS) / 2;
#[allow(dead_code)]
const LOAD_PROGRESS_LOOP_INITIAL_NETWORK_BUILDINGS: u32 = LOAD_PROGRESS_MAX_ALL_THE_FREAKN_OBJECTS + 1;
#[allow(dead_code)]
const LOAD_PROGRESS_POST_INITIAL_NETWORK_BUILDINGS: u32 = LOAD_PROGRESS_LOOP_INITIAL_NETWORK_BUILDINGS + 8 + 1;
#[allow(dead_code)]
const LOAD_PROGRESS_POST_PRELOAD_ASSETS: u32 = LOAD_PROGRESS_POST_INITIAL_NETWORK_BUILDINGS + 1;
#[allow(dead_code)]
const LOAD_PROGRESS_POST_STARTING_CAMERA: u32 = LOAD_PROGRESS_POST_PRELOAD_ASSETS + 1;
#[allow(dead_code)]
const LOAD_PROGRESS_POST_STARTING_CAMERA_2: u32 = LOAD_PROGRESS_POST_STARTING_CAMERA + 1;
const LOAD_PROGRESS_END: u32 = 100;

/// One step of the staged build, in the order `GameLogic::startNewGame` performs the work.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// `LOAD_PROGRESS_START` / `POST_PARTICLE_INI_LOAD`: VFS + INI + audio + FX database + map.
    Init,
    /// `LOAD_PROGRESS_SIDE_POPULATION`..`POST_PATHFINDER`: start the skirmish (`GameLogic` side /
    /// player / script setup, the map's logic objects and starting units).
    Sim,
    /// `POST_TERRAIN_LOGIC_NEW_MAP`: terrain mesh + texture atlas + shroud overlay.
    Terrain,
    /// `POST_BRIDGE_LOAD`: roads, bridges, water, sky, shadow assets.
    Ground,
    /// `MAX_ALL_THE_FREAKN_OBJECTS`: static map objects (props/trees), start markers, object lights.
    Objects,
    /// `POST_STARTING_CAMERA`..`END`: start camera, resources, install the sim, hand over to `InGame`.
    Settle,
    Done,
}

/// Everything the staged build carries between frames. Plain `Resource`; the non-send [`SimInit`]
/// lives in [`PendingSim`].
#[derive(Resource)]
pub struct BuildJob {
    stage: Stage,
    config: GameConfig,
    sim_args: SimArgs,
    color_table: Vec<[u8; 3]>,
    vfs: Option<Vfs>,
    ini: Option<IniDb>,
    map: Option<MapFile>,
    playback: Option<crate::sim_view::ReplayPlayback>,
    anim_cache: anim::AnimCache,
    fx_db: Option<Arc<zr_fx::FxDatabase>>,
    fx_assets: crate::fx::FxAssets,
    fx_rng: Option<zr_fx::ClientRng>,
    map_light: Option<GlobalLighting>,
}

/// The started-but-not-installed sim (non-send: it owns `dyn` modules).
#[derive(Default)]
pub struct PendingSim(pub Option<SimInit>);

impl BuildJob {
    fn new(config: GameConfig, sim_args: SimArgs, color_table: Vec<[u8; 3]>, playback: Option<crate::sim_view::ReplayPlayback>) -> Self {
        Self {
            stage: Stage::Init,
            config,
            sim_args,
            color_table,
            vfs: None,
            ini: None,
            map: None,
            playback,
            anim_cache: anim::AnimCache::default(),
            fx_db: None,
            fx_assets: crate::fx::FxAssets::default(),
            fx_rng: None,
            map_light: None,
        }
    }
}

pub fn add_systems(app: &mut App) {
    app.insert_non_send(PendingSim::default());
    app.add_systems(OnEnter(ShellState::Loading), setup_build);
    app.add_systems(Update, loading_build.run_if(in_state(ShellState::Loading)).run_if(|args: Res<Args>| !args.load_screen));
}

/// `OnEnter(Loading)`: create the job from the pending game. A `--replay` header overrides the
/// setup's config (as the old `run_build` did).
fn setup_build(mut commands: Commands, pending: Res<PendingGame>, args: Res<Args>, shell_data: Option<Res<ShellData>>) {
    let mut config = pending.0.clone();
    let mut playback = None;
    if let Some(path) = &args.replay {
        match crate::load_replay(path) {
            Ok(rep) => {
                info!("replay: loaded {} ({} events, {} CRCs, seed {})", rep.header.map, rep.events.len(), rep.crcs.len(), rep.header.seed);
                config = GameConfig::from_replay_header(&rep.header);
                playback = Some(crate::sim_view::ReplayPlayback::from_replay(&rep));
            }
            Err(e) => eprintln!("zr-viewer: replay: {e:#}"),
        }
    }
    let color_table = shell_data.map(|d| crate::shell::color_table(&d)).unwrap_or_default();
    commands.insert_resource(BuildJob::new(config, args.sim.clone(), color_table, playback));
}

/// Advance the build by one stage and report the matching `LOAD_PROGRESS_*` checkpoint. The final
/// stage installs the sim and switches to `InGame`.
#[allow(clippy::too_many_arguments)]
fn loading_build(
    mut commands: Commands,
    mut job: ResMut<BuildJob>,
    mut pending_sim: NonSendMut<PendingSim>,
    mut slot: NonSendMut<SimSlot>,
    mut load: ResMut<LoadProgress>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut terrain_materials: ResMut<Assets<terrain::TerrainMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut inv_bind: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut next: ResMut<NextState<ShellState>>,
    args: Res<Args>,
    mut scorches: ResMut<crate::scorch::Scorches>,
    settings: Option<Res<crate::options::GameSettings>>,
) {
    let BuildJob { stage, config, sim_args, color_table, vfs, ini, map, playback, anim_cache, fx_db, fx_assets, fx_rng, map_light } = &mut *job;
    match stage {
        Stage::Init => {
            let Some(install) = zr_assets::Installs::detect().zero_hour else {
                eprintln!("zr-viewer: Zero Hour install not found (set ZR_ZEROHOUR_DIR)");
                std::process::exit(1);
            };
            let v = match Vfs::zero_hour(&install) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("zr-viewer: could not mount Zero Hour VFS: {e:#}");
                    std::process::exit(1);
                }
            };
            let t0 = std::time::Instant::now();
            let i = zr_ini::load_zero_hour(&v).db;
            let audio = crate::audio::AudioBank::new(&i, &zr_text::detect_language(&v), args.mute || args.screenshot.is_some());
            commands.insert_resource(audio);
            let presets = crate::options::LodPresets::from_ini(&i);
            // The launch `TextureReduction` needs the detail presets (`GameLOD.cpp:565-580`); the world's textures
            // are built from here on.
            if let Some(s) = settings.as_deref() {
                crate::options::apply_texture_reduction(s, &presets);
            }
            commands.insert_resource(presets);
            commands.insert_resource(crate::options::InstallDefaults::from_ini(&i));
            commands.insert_resource(crate::eva::build(&i));
            let fxd = Arc::new(zr_fx::FxDatabase::from_ini(&i));
            info!("fx: {} FXLists, {} ParticleSystems", fxd.fx_list_count(), fxd.particle_count());
            let fx_seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
            *fx_rng = Some(zr_fx::ClientRng::new(fx_seed));
            let m = match crate::load_map(&v, &config.map) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("zr-viewer: {e:#}");
                    std::process::exit(1);
                }
            };
            if let Some(hm) = m.height_map.as_ref() {
                info!("loaded ini + map in {:?}: {}x{} cells, border {}, {} objects", t0.elapsed(), hm.width, hm.height, hm.border_size, m.objects.len());
            }
            if args.list {
                let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
                for o in &m.objects {
                    *counts.entry(&o.template).or_default() += 1;
                }
                for (t, n) in counts {
                    println!("{n:5} {t}");
                }
                for o in m.objects.iter().filter(|o| o.template.to_ascii_lowercase().contains("waypoint")) {
                    println!("waypoint {:?} at {:?}", o.props.str("waypointName"), o.pos);
                }
                std::process::exit(0);
            }
            *fx_db = Some(fxd);
            *vfs = Some(v);
            *ini = Some(i);
            *map = Some(m);
            *stage = Stage::Sim;
            load.0 = LOAD_PROGRESS_POST_LOAD_MAP;
        }
        Stage::Sim => {
            let v = vfs.as_ref().unwrap();
            let sim = SimInit::new(v, &config.map, &config.slots, color_table, config.seed, sim_args.clone());
            pending_sim.0 = sim;
            *stage = Stage::Terrain;
            load.0 = LOAD_PROGRESS_POST_PATHFINDER_NEW_MAP;
        }
        Stage::Terrain => {
            let v = vfs.as_ref().unwrap();
            let i = ini.as_ref().unwrap();
            let m = map.as_ref().unwrap();
            let hm = m.height_map.as_ref().unwrap();
            let Ok(mut t) = source::terrain(m, v, i) else {
                eprintln!("zr-viewer: terrain failed");
                std::process::exit(1);
            };
            let light = lighting::map_lighting(m);
            lighting::bake_terrain(&mut t, &light, Ground::new(hm).playable_size());
            let shadow_color = lighting::shadow_color_linear(&light);
            let pieces = convert::terrain(&t, &mut images, shadow_color);
            commands.spawn((
                Mesh3d(meshes.add(pieces.mesh)),
                MeshMaterial3d(terrain_materials.add(pieces.material)),
                Transform::IDENTITY,
                bevy::light::NotShadowCaster,
            ));
            // The 3-way blend pass (`HeightMap.cpp` `renderExtraBlendTiles`).
            if let Some((mesh, material)) = pieces.extra {
                commands.spawn((
                    Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(terrain_materials.add(material)),
                    Transform::IDENTITY,
                    bevy::light::NotShadowCaster,
                ));
            }
            if let Some(sim) = pending_sim.0.as_ref() {
                let logic = sim.game().logic();
                let (cx, cy) = logic.partition().cell_counts();
                let cell_size = logic.partition().cell_size();
                let d = sim.game().data();
                let view = crate::shroud::spawn_overlay(
                    &mut commands,
                    &mut meshes,
                    &mut materials,
                    &mut images,
                    &t,
                    (cx, cy),
                    cell_size,
                    d.shroud_color,
                    d.clear_alpha,
                    d.fog_alpha,
                    d.shroud_alpha,
                );
                commands.insert_resource(view);
            }
            *map_light = Some(light);
            *stage = Stage::Ground;
            load.0 = LOAD_PROGRESS_LOOP_ALL_THE_FREAKN_OBJECTS;
        }
        Stage::Ground => {
            let v = vfs.as_ref().unwrap();
            let i = ini.as_ref().unwrap();
            let m = map.as_ref().unwrap();
            let hm = m.height_map.as_ref().unwrap();
            let light = map_light.as_ref().unwrap();
            let ground = Ground::new(hm);
            crate::roads::spawn(&mut commands, &mut meshes, &mut materials, &mut terrain_materials, &mut images, m, v, i, &ground, light);
            crate::water::spawn(&mut commands, &mut meshes, &mut materials, &mut images, m, v, i, &ground, light);
            {
                let (pw, ph) = ground.playable_size();
                let settings = crate::sky::SkySettings::from_ini(i);
                crate::sky::spawn(&mut commands, &mut meshes, &mut materials, &mut images, v, settings, (pw * pw + ph * ph).sqrt() * 0.5);
            }
            let tod = light.tod_index();
            let sun = convert::to_bevy(light.objects[tod][0].direction).normalize_or_zero();
            commands.insert_resource(crate::shadow::spawn_assets(&mut meshes, sun, light.shadow_color));
            *stage = Stage::Objects;
            load.0 = LOAD_PROGRESS_OBJECT_LOOP_MID;
        }
        Stage::Objects => {
            let v = vfs.as_ref().unwrap();
            let i = ini.as_ref().unwrap();
            let m = map.as_ref().unwrap();
            let hm = m.height_map.as_ref().unwrap();
            let light = map_light.as_ref().unwrap();
            let ground = Ground::new(hm);
            type Parts = Vec<(Handle<Mesh>, Handle<StandardMaterial>)>;
            let teams = crate::team_color::TeamColors::new(m, i);
            let mut raw: std::collections::HashMap<String, Option<Vec<source::ModelPart>>> = std::collections::HashMap::new();
            let mut cache: std::collections::HashMap<(String, [u8; 3]), Parts> = std::collections::HashMap::new();
            let mut spawned = 0usize;
            let mut house_colors: std::collections::BTreeMap<(String, [u8; 3]), usize> = Default::default();
            for o in m.objects.iter().filter(|o| source::is_visible(o) && !pending_sim.0.as_ref().is_some_and(|s| s.is_logic(o))) {
                let Some(src) = raw.entry(o.template.clone()).or_insert_with(|| source::object_model(i, v, &o.template)) else { continue };
                let house = src.iter().any(crate::team_color::uses_house_color);
                let rgb = if house { teams.object_color(o) } else { crate::team_color::NEUTRAL };
                if house {
                    *house_colors.entry((o.template.clone(), rgb)).or_default() += 1;
                }
                let asset = anim_cache.asset(i, v, &mut inv_bind, &o.template);
                let pivots = asset.as_ref().map(|a| a.hierarchy.pivot_count());
                let parts = cache.entry((o.template.clone(), rgb)).or_insert_with(|| {
                    src.iter()
                        .map(|p| {
                            let tinted;
                            let p = if house && crate::team_color::uses_house_color(p) {
                                tinted = crate::team_color::tint(p, rgb);
                                &tinted
                            } else {
                                p
                            };
                            let (mesh, mat) = convert::model_part(p, &mut images, pivots);
                            (meshes.add(mesh), materials.add(mat))
                        })
                        .collect()
                });
                let mut pos = o.pos;
                pos[2] += ground.height(pos[0], pos[1]);
                if o.template.to_ascii_lowercase().contains("tree") {
                    debug!("tree {} at {:?}", o.template, o.pos);
                }
                let t = convert::object_transform(pos, o.angle);
                let root = commands.spawn((t, Visibility::default())).id();
                let part_entities = anim::populate(&mut commands, root, src.as_slice(), parts.as_slice(), asset.as_ref(), &zr_w3d::ConditionFlags::new(), anim_cache, v);
                let shroud_parts = part_entities.into_iter().zip(parts.iter().map(|(_, mat)| mat.clone())).collect();
                commands.entity(root).insert(crate::terrain_shroud::ShroudProp::new(pos, shroud_parts));
                spawned += 1;
            }
            info!("spawned {spawned} objects; house-colour objects (template, rgb): {house_colors:?}");
            let mut start_mats: std::collections::HashMap<u32, (Handle<Mesh>, Handle<StandardMaterial>)> = std::collections::HashMap::new();
            for (n, mut pos) in source::player_starts(m).into_iter().filter(|_| pending_sim.0.is_none()) {
                info!("Player_{n}_Start at {pos:?}");
                pos[2] += ground.height(pos[0], pos[1]);
                let col = match n {
                    1 => [1.0, 0.0, 0.0, 1.0],
                    2 => [0.0, 0.2, 1.0, 1.0],
                    _ => [0.0, 1.0, 0.0, 1.0],
                };
                let (mesh, mat) = start_mats.entry(n).or_insert_with(|| {
                    let (mesh, mat) = convert::model_part(&source::marker(col), &mut images, None);
                    (meshes.add(mesh), materials.add(mat))
                });
                commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(mat.clone()), convert::object_transform(pos, 0.0)));
            }
            lighting::spawn_object_lights(&mut commands, &mut ambient, light);
            *stage = Stage::Settle;
            load.0 = LOAD_PROGRESS_MAX_ALL_THE_FREAKN_OBJECTS;
        }
        Stage::Settle => {
            let (Some(v), Some(i), Some(m)) = (vfs.take(), ini.take(), map.take()) else { return };
            let hm = m.height_map.as_ref().unwrap();
            let ground = Ground::new(hm);
            // `--spawn` debug model, before the camera picks its focus.
            let demo = args.spawn.as_ref().map(|template| {
                let (pw, ph) = ground.playable_size();
                let mut pos = [pw * 0.45, ph * 0.35, 0.0];
                pos[2] += ground.height(pos[0], pos[1]);
                let asset = anim_cache.asset(&i, &v, &mut inv_bind, template);
                let pivots = asset.as_ref().map(|a| a.hierarchy.pivot_count());
                let src = source::object_model(&i, &v, template).unwrap_or_default();
                let handles: Vec<(Handle<Mesh>, Handle<StandardMaterial>)> = src
                    .iter()
                    .map(|p| {
                        let (mesh, mat) = convert::model_part(p, &mut images, pivots);
                        (meshes.add(mesh), materials.add(mat))
                    })
                    .collect();
                let root = commands.spawn((convert::object_transform(pos, 0.0), Visibility::default())).id();
                anim::populate(&mut commands, root, &src, &handles, asset.as_ref(), &zr_w3d::ConditionFlags::new(), anim_cache, &v);
                info!("spawned demo {template} at {pos:?}");
                convert::to_bevy(pos)
            });
            let (pw, ph) = ground.playable_size();
            let centre = Vec3::new(pw / 2.0, 0.0, -ph / 2.0);
            let focus = demo
                .or_else(|| if args.overview { Some(centre) } else { pending_sim.0.as_ref().and_then(|s| s.focus()) })
                .unwrap_or(centre);
            if let Some(name) = &args.fx {
                let at = convert::from_bevy(focus);
                let fxd = fx_db.as_ref().unwrap();
                let n = crate::fx::spawn_fx_list(
                    &mut commands,
                    &mut meshes,
                    &mut materials,
                    &mut images,
                    &v,
                    fxd,
                    fx_assets,
                    fx_rng.as_mut().unwrap(),
                    name,
                    at,
                    [0.0, 0.0, 0.0],
                    crate::fx::Aim::default(),
                    Some(&ground),
                    &crate::fx::BonePositions::new(),
                    &mut scorches,
                );
                info!("fx: spawned {n} systems for {name} at {at:?}");
            }
            commands.insert_resource(crate::fx::FxDb(fx_db.take().unwrap()));
            commands.insert_resource(std::mem::take(fx_assets));
            commands.insert_resource(crate::fx::FxRng(fx_rng.take().unwrap()));
            let rts = if let Some((cx, cy, dist)) = args.camera {
                // `--camera x,y,dist`: focus in Generals space (ground height at x,y).
                let z = ground.height(cx, cy);
                crate::camera::RtsCam {
                    focus: convert::to_bevy([cx, cy, z]),
                    yaw: 0.0,
                    pitch: crate::camera::RTS_PITCH,
                    dist,
                    min_dist: 5.0,
                    max_dist: 4000.0,
                }
            } else if demo.is_some() || args.fx.is_some() || sim_args.duel || sim_args.ghost_view {
                let dist = if demo.is_some() {
                    45.0
                } else if sim_args.duel {
                    130.0
                } else if sim_args.ghost_view {
                    220.0
                } else {
                    70.0
                };
                crate::camera::RtsCam { focus, yaw: 0.0, pitch: crate::camera::RTS_PITCH, dist, min_dist: 10.0, max_dist: 400.0 }
            } else if args.overview {
                let span = pw.max(ph);
                crate::camera::RtsCam { focus: centre, yaw: 0.0, pitch: 70f32.to_radians(), dist: span * 1.25, min_dist: 100.0, max_dist: span * 3.0 }
            } else {
                crate::camera::RtsCam { focus, yaw: 0.0, pitch: crate::camera::RTS_PITCH, dist: 450.0, min_dist: 60.0, max_dist: pw.max(ph) * 3.0 }
            };
            let t = crate::camera::rts_transform(&rts);
            commands.spawn((
                Camera3d::default(),
                bevy::core_pipeline::tonemapping::Tonemapping::None,
                Projection::Perspective(PerspectiveProjection { far: 20000.0, near: 5.0, ..default() }),
                t,
                bevy::audio::SpatialListener::new(4.0),
            ));
            commands.insert_resource(rts);
            let _ = source::MAP_XY_FACTOR;
            if let Some(sim) = pending_sim.0.take() {
                let mut installed = sim.install(&mut commands, v, i, &m, &mut meshes, &mut materials);
                if args.record.is_some() {
                    let header = zr_game::replay::ReplayHeader {
                        map: config.map.clone(),
                        seed: config.seed,
                        starting_cash: config.starting_cash,
                        superweapon_restriction: config.superweapon_restriction,
                        slots: config.slots.clone(),
                    };
                    installed.game.start_recording(header, 30);
                    info!("replay: recording {} slots, seed {}", config.slots.len(), config.seed);
                }
                slot.0 = Some(installed);
            }
            commands.insert_resource(MapRes(m));
            commands.insert_resource(std::mem::take(anim_cache));
            commands.insert_resource(CurrentGame(config.clone()));
            if let Some(pb) = playback.take() {
                commands.insert_resource(pb);
            }
            commands.remove_resource::<PendingGame>();
            *stage = Stage::Done;
            load.0 = LOAD_PROGRESS_END;
            next.set(ShellState::InGame);
        }
        Stage::Done => {}
    }
    debug!("load progress {}", load.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stages must advance the bar monotonically (they replace the old fixed ramp).
    #[test]
    fn load_progress_checkpoints_are_monotonic() {
        let seq = [
            LOAD_PROGRESS_POST_LOAD_MAP,
            LOAD_PROGRESS_POST_PATHFINDER_NEW_MAP,
            LOAD_PROGRESS_LOOP_ALL_THE_FREAKN_OBJECTS,
            LOAD_PROGRESS_OBJECT_LOOP_MID,
            LOAD_PROGRESS_MAX_ALL_THE_FREAKN_OBJECTS,
            LOAD_PROGRESS_END,
        ];
        assert!(seq.windows(2).all(|w| w[0] <= w[1]), "{seq:?}");
        assert_eq!(seq[seq.len() - 1], 100);
    }
}
