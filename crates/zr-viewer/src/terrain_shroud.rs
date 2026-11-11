//! Shroud darkening for terrain-attached geometry (roads, water, bridges, props and trees).
//!
//! The terrain itself is darkened by the [`crate::shroud`] overlay mesh, but retail shades
//! everything that sits on the ground too, each with its own projected shroud pass:
//!
//! * roads are darkened by the terrain's shroud multiply pass, which redraws the terrain over the
//!   (depth-write-disabled) road decals (`HeightMap.cpp` `renderTerrainPass`),
//! * rivers bake the shroud level into their vertex diffuse (`W3DWater.cpp`
//!   `getRiverVertexDiffuse`) and water areas modulate by the shroud texture in-pass
//!   (`W3DWater.cpp`),
//! * bridges get a second `ST_SHROUD_TEXTURE` pass (`W3DBridgeBuffer::drawBridges`),
//! * props (including trees) are drawn with a shroud material pass while fogged and skipped once
//!   fully shrouded (`W3DPropBuffer::drawProps`), and trees via the tree buffer's projected shroud
//!   texture (`W3DTreeBuffer::drawTrees`).
//!
//! The viewer reproduces this without a per-fragment projection shader:
//!
//! * roads/water carry per-vertex colours already, so the local player's cell shroud level is
//!   multiplied into them each frame ([`ShroudVertices`]),
//! * bridges and props/trees swap to a darkened copy of their material while fogged and hide once
//!   fully shrouded ([`ShroudProp`]).
//!
//! Approximation: retail projects the shroud per fragment; the viewer classifies a bridge / prop /
//! tree by the cell at its centre, so a large object straddling a fog boundary darkens or hides as
//! a whole. Roads and water keep their per-vertex granularity.
//!
//! Client-only; the sim owns the shroud grid ([`zr_sim::shroud`]).

use std::collections::HashMap;

use bevy::prelude::*;
use zr_sim::Coord3D;

use crate::sim_view::{SimActive, SimRes};

/// The per-vertex colour factor the local player's shroud applies at one world position.
#[derive(Clone, Copy)]
pub struct ShroudFactor(pub [f32; 3]);

impl ShroudFactor {
    fn new(color: [u8; 3], alpha: u8) -> Self {
        let s = f32::from(alpha) / 255.0;
        Self([f32::from(color[0]) / 255.0 * s, f32::from(color[1]) / 255.0 * s, f32::from(color[2]) / 255.0 * s])
    }
}

/// `GameData` colours + the local cell status -> the multiplier retail's shroud pass would apply.
#[derive(Clone, Copy)]
struct ShroudLookup {
    color: [u8; 3],
    clear_alpha: u8,
    fog_alpha: u8,
    shroud_alpha: u8,
}

impl ShroudLookup {
    fn factor(&self, status: zr_sim::shroud::CellShroudStatus) -> [f32; 3] {
        use zr_sim::shroud::CellShroudStatus::*;
        let alpha = match status {
            Clear => self.clear_alpha,
            Fogged => self.fog_alpha,
            Shrouded => self.shroud_alpha,
        };
        ShroudFactor::new(self.color, alpha).0
    }
}

/// A mesh whose `ATTRIBUTE_COLOR` carries the terrain/water look: the shroud factor is multiplied
/// into it each frame (roads, water). `base`/`positions` are the unshrouded colours and the
/// Generals-space vertex positions used to look up the local player's cell.
#[derive(Component)]
pub struct ShroudVertices {
    mesh: Handle<Mesh>,
    base: Vec<[f32; 4]>,
    positions: Vec<[f32; 3]>,
}

impl ShroudVertices {
    pub fn new(mesh: Handle<Mesh>, base: Vec<[f32; 4]>, positions: Vec<[f32; 3]>) -> Self {
        Self { mesh, base, positions }
    }
}

/// Which rendering the local player currently gives one static bridge / prop / tree.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum PropState {
    #[default]
    Clear,
    Fogged,
    Shrouded,
}

/// A static, terrain-attached object (bridge piece, prop, tree): `parts` are its `MeshMaterial3d`
/// entities with their unshrouded material handles. Fogged -> darken, shrouded -> hide
/// (`W3DPropBuffer::drawProps`).
#[derive(Component, Default)]
pub struct ShroudProp {
    pos: [f32; 3],
    state: PropState,
    parts: Vec<(Entity, Handle<StandardMaterial>)>,
}

impl ShroudProp {
    /// `pos` is in Generals space (as map objects / bridge pieces are stored).
    pub fn new(pos: [f32; 3], parts: Vec<(Entity, Handle<StandardMaterial>)>) -> Self {
        Self { pos, state: PropState::Clear, parts }
    }
}

/// Darkened copies of prop materials, keyed by the unshrouded handle (one clone per template
/// material, shared by every fogged instance).
#[derive(Resource, Default)]
pub(crate) struct ShroudTintCache(HashMap<Handle<StandardMaterial>, Handle<StandardMaterial>>);

pub fn add_systems(app: &mut App) {
    app.init_resource::<ShroudTintCache>();
    app.add_systems(
        Update,
        (update_prop_shroud, update_vertex_shroud)
            .run_if(resource_exists::<SimActive>)
            .run_if(in_state(crate::shell::ShellState::InGame))
            .after(crate::sim_view::sim_step),
    );
}

fn lookup(sim: &SimRes) -> ShroudLookup {
    let d = sim.game.data();
    ShroudLookup { color: d.shroud_color, clear_alpha: d.clear_alpha, fog_alpha: d.fog_alpha, shroud_alpha: d.shroud_alpha }
}

/// Hide fully shrouded bridges/props/trees and darken fogged ones, per the local player's grid.
fn update_prop_shroud(
    sim: NonSend<SimRes>,
    mut cache: ResMut<ShroudTintCache>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
    mut props: Query<(&mut ShroudProp, &mut Visibility)>,
) {
    let look = lookup(&sim);
    let grid = sim.game.logic().shroud();
    for (mut prop, mut vis) in &mut props {
        let status = grid.cell_status_at(sim.local, &Coord3D::new(prop.pos[0], prop.pos[1], prop.pos[2]));
        let state = match status {
            zr_sim::shroud::CellShroudStatus::Clear => PropState::Clear,
            zr_sim::shroud::CellShroudStatus::Fogged => PropState::Fogged,
            zr_sim::shroud::CellShroudStatus::Shrouded => PropState::Shrouded,
        };
        if state == prop.state {
            continue;
        }
        prop.state = state;
        match state {
            PropState::Shrouded => *vis = Visibility::Hidden,
            PropState::Clear => {
                *vis = Visibility::Inherited;
                for (e, orig) in &prop.parts {
                    commands.entity(*e).insert(MeshMaterial3d(orig.clone()));
                }
            }
            PropState::Fogged => {
                *vis = Visibility::Inherited;
                let tint = ShroudFactor::new(look.color, look.fog_alpha).0;
                for (e, orig) in &prop.parts {
                    let dark = cache.0.entry(orig.clone()).or_insert_with(|| {
                        let mut m = materials.get(orig).cloned().unwrap_or_default();
                        m.base_color = Color::linear_rgba(
                            m.base_color.to_linear().red * tint[0],
                            m.base_color.to_linear().green * tint[1],
                            m.base_color.to_linear().blue * tint[2],
                            m.base_color.to_linear().alpha,
                        );
                        materials.add(m)
                    });
                    commands.entity(*e).insert(MeshMaterial3d(dark.clone()));
                }
            }
        }
    }
}

/// Multiply the local player's shroud level into roads'/water's per-vertex colours.
fn update_vertex_shroud(sim: NonSend<SimRes>, mut meshes: ResMut<Assets<Mesh>>, q: Query<&ShroudVertices>) {
    let look = lookup(&sim);
    let grid = sim.game.logic().shroud();
    for sv in &q {
        let next: Vec<[f32; 4]> = sv
            .positions
            .iter()
            .zip(&sv.base)
            .map(|(p, b)| {
                let status = grid.cell_status_at(sim.local, &Coord3D::new(p[0], p[1], p[2]));
                let f = look.factor(status);
                [b[0] * f[0], b[1] * f[1], b[2] * f[2], b[3]]
            })
            .collect();
        // Compare before `get_mut` so an unchanged mesh is not marked modified (which would
        // re-upload it to the GPU every frame).
        let changed = match meshes.get(&sv.mesh).and_then(|m| m.attribute(Mesh::ATTRIBUTE_COLOR)) {
            Some(bevy::mesh::VertexAttributeValues::Float32x4(v)) => *v != next,
            Some(_) => true,
            None => true,
        };
        if !changed {
            continue;
        }
        if let Some(mut mesh) = meshes.get_mut(&sv.mesh) {
            mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, next);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zr_sim::shroud::CellShroudStatus;

    #[test]
    fn factor_matches_the_shroud_levels() {
        let l = ShroudLookup { color: [255, 255, 255], clear_alpha: 255, fog_alpha: 127, shroud_alpha: 0 };
        assert_eq!(l.factor(CellShroudStatus::Clear), [1.0, 1.0, 1.0]);
        assert_eq!(l.factor(CellShroudStatus::Shrouded), [0.0, 0.0, 0.0]);
        let fog = l.factor(CellShroudStatus::Fogged);
        assert!((fog[0] - 127.0 / 255.0).abs() < 1e-6, "{fog:?}");
        // `ShroudColor` tints the multiplied value.
        let red = ShroudLookup { color: [255, 0, 0], ..l };
        assert_eq!(red.factor(CellShroudStatus::Fogged), [127.0 / 255.0, 0.0, 0.0]);
    }
}
