//! Rally point routes and markers of the selected factories.
//!
//! Ported from `W3DWaypointBuffer::drawWaypoints` (`GeneralsMD/Code/GameEngineDevice/Source/W3DDevice/
//! GameClient/W3dWaypointBuffer.cpp`, the branch for selected drawables with an `ExitInterface`): a line
//! from the unit create point, through the natural rally point, to the rally point, in the default
//! waypoint style (`setDefaultLineStyle`: colour (0.25, 0.5, 1.0), additive, width 1.5). The `SCMNode`
//! model ("the little hockey puck") is drawn at the natural rally point (`:520`). The elbows of the
//! box-wrap case (`:380-510`) are not drawn: the route is the straight polyline through its three points.
//!
//! The marker at the rally point is the `RallyPointMarker` object (`Object RallyPointMarker` in
//! `INIZH.big`: a `W3DModelDraw` with the looping `SCMRally` model, `DRAWABLE_ONLY`). `ControlBar::showRallyPoint`
//! (`ControlBar.cpp:2693`) creates it at the rally point, oriented to `GlobalData::m_downwindAngle` and
//! tinted with the local player's colour, while the control bar shows a single selected object that has a
//! rally point (`ControlBarCommand.cpp:505`).
//!
//! Simplification: the route is a flat ribbon on the ground rather than the screen-space segmented line.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use zr_sim::Coord3D;

use crate::anim::AnimCache;
use crate::construct_modules::{self, ExtraCache, Gfx, PosedSpec};
use crate::convert::{self, to_bevy};
use crate::sim_view::{ModelCache, SimRes};

/// Half the width of the route ribbon (world units).
const HALF_WIDTH: f32 = 0.75;
/// Lift above the ground so the ribbon does not z-fight the terrain.
const LIFT: f32 = 0.6;
/// `setDefaultLineStyle`'s colour (0.25, 0.5, 1.0), additive.
const ROUTE_COLOR: Color = Color::srgb(0.25, 0.5, 1.0);
/// `GlobalData::m_downwindAngle` default (`GlobalData.cpp:630`, "Northeast!"), radians.
const DOWNWIND_ANGLE: f32 = -0.785;

/// The drawn route meshes and markers; despawned and rebuilt when the routes change.
#[derive(Component)]
pub(crate) struct RallyRoute;

/// The routes currently drawn, as exact float bits so a change is always detected.
#[derive(Resource, Default)]
pub(crate) struct RallyState {
    key: Vec<(u32, Vec<[u32; 3]>)>,
}

pub fn add_systems(app: &mut App) {
    app.init_resource::<RallyState>();
    // `AnimCache` arrives with the game's staged build (`build.rs`), which can finish after the state changes.
    app.add_systems(
        Update,
        rally_sync
            .after(crate::sim_view::sim_step)
            .run_if(in_state(crate::shell::ShellState::InGame))
            .run_if(resource_exists::<AnimCache>),
    );
}

/// Mesh buffers for the route ribbon.
#[derive(Default)]
struct Buf {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

impl Buf {
    fn quad(&mut self, corners: [[f32; 3]; 4]) {
        let base = self.positions.len() as u32;
        for c in corners {
            self.positions.push(to_bevy(c).to_array());
            self.normals.push([0.0, 1.0, 0.0]);
            self.colors.push([1.0; 4]);
        }
        self.indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    fn mesh(self) -> Mesh {
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.colors)
            .with_inserted_indices(Indices::U32(self.indices))
    }
}

/// Draws the rally route of every selected factory the local player owns and has given a rally point, and
/// the rally marker of a single selected one.
#[allow(clippy::too_many_arguments)]
fn rally_sync(
    mut commands: Commands,
    sim: Option<NonSend<SimRes>>,
    mut state: ResMut<RallyState>,
    old: Query<Entity, With<RallyRoute>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut inv_bind: ResMut<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    cache: Option<Res<ModelCache>>,
    mut anim_cache: ResMut<AnimCache>,
    mut cx: ResMut<ExtraCache>,
) {
    let Some(sim) = sim else { return };
    let local = sim.local;
    let logic = sim.game.logic();
    let mut routes: Vec<(u32, Vec<Coord3D>)> = Vec::new();
    for &id in &sim.selected {
        if !logic.find_object(id).is_some_and(|o| o.player == local) {
            continue;
        }
        if let Some(route) = logic.rally_route(id) {
            routes.push((id.0, route));
        }
    }
    let key: Vec<(u32, Vec<[u32; 3]>)> = routes
        .iter()
        .map(|(id, r)| (*id, r.iter().map(|p| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]).collect()))
        .collect();
    if key == state.key && old.is_empty() == routes.is_empty() {
        return;
    }
    for e in &old {
        commands.entity(e).despawn();
    }
    state.key = key;
    if routes.is_empty() {
        return;
    }

    let mut route_buf = Buf::default();
    for (_, route) in &routes {
        for pair in route.windows(2) {
            push_segment(&mut route_buf, pair[0], pair[1]);
        }
    }
    if !route_buf.indices.is_empty() {
        let material = materials.add(StandardMaterial {
            base_color: ROUTE_COLOR,
            alpha_mode: AlphaMode::Add,
            unlit: true,
            cull_mode: None,
            double_sided: true,
            ..default()
        });
        let mesh = meshes.add(route_buf.mesh());
        commands.spawn((Mesh3d(mesh), MeshMaterial3d(material), Transform::IDENTITY, Visibility::default(), RallyRoute));
    }

    let Some(cache) = cache else { return };
    let mut gfx = Gfx { meshes: &mut meshes, materials: &mut materials, images: &mut images, inv_bind: &mut inv_bind };
    // `SCMNode` at the natural rally point (`W3dWaypointBuffer.cpp:520`), in the model's own colours.
    for (_, route) in &routes {
        let Some(&nrp) = route.get(1) else { continue };
        let puck = PosedSpec {
            model: "SCMNode",
            clip: None,
            mode: zr_w3d::AnimMode::Once,
            start_last: false,
            rgb: crate::team_color::NEUTRAL,
            hide_show: &[],
            transform: convert::object_transform([nrp.x, nrp.y, nrp.z], 0.0),
        };
        let root = commands.spawn((RallyRoute, Transform::IDENTITY, Visibility::default())).id();
        construct_modules::spawn_posed(&mut commands, root, &mut cx, &mut anim_cache, &cache.vfs, &mut gfx, &puck);
    }

    // The marker at the single selected factory's rally point (`ControlBar::showRallyPoint`).
    // `ControlBarCommand.cpp:505` shows it for the single selection only.
    if let (1, Some(&end)) = (sim.selected.len(), routes.first().and_then(|(_, r)| r.last())) {
        let Some((model, clip, mode, start_last)) = construct_modules::default_state_spec(&mut anim_cache, &cache.ini, "RallyPointMarker") else {
            return;
        };
        let marker = PosedSpec {
            model: &model,
            clip: clip.as_deref(),
            mode,
            start_last,
            rgb: cache.color_of(local),
            hide_show: &[],
            transform: Transform::IDENTITY,
        };
        let root = commands.spawn((RallyRoute, convert::object_transform([end.x, end.y, end.z], DOWNWIND_ANGLE), Visibility::default())).id();
        construct_modules::spawn_posed(&mut commands, root, &mut cx, &mut anim_cache, &cache.vfs, &mut gfx, &marker);
    }
}

/// A flat quad of half-width [`HALF_WIDTH`] along `a`..`b`, lifted by [`LIFT`].
fn push_segment(buf: &mut Buf, a: Coord3D, b: Coord3D) {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = (dx * dx + dy * dy).sqrt();
    if len <= f32::EPSILON {
        return;
    }
    // Perpendicular in the ground plane (Generals X/Y), so the ribbon lies flat.
    let (px, py) = (-dy / len * HALF_WIDTH, dx / len * HALF_WIDTH);
    buf.quad([
        [a.x + px, a.y + py, a.z + LIFT],
        [a.x - px, a.y - py, a.z + LIFT],
        [b.x - px, b.y - py, b.z + LIFT],
        [b.x + px, b.y + py, b.z + LIFT],
    ]);
}
