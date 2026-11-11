//! Client visuals for the Chinook combat drop (`ChinookAIUpdate` / `AIRappelState`): the ropes the
//! passengers slide down, plus the `RAPPELLING` model condition on a descending infantryman.
//!
//! Read-only from the sim: [`zr_sim::GameLogic::chinook_state`] exposes each rope's top
//! (`RopeStart`, approximated by the sim) and its current `len`/`len_max` in world units, and
//! [`zr_sim::GameLogic::rappel_state`] marks a passenger that is sliding down. The engine draws the
//! ropes from the model's `RopeStart`/`RopeEnd` bones (`W3DChinookDraw`'s rope drawables); the sim
//! seeds its rope tops along the Chinook's belly (documented reduction in `zr-sim::chinook`), so the
//! viewer draws a thin line from each rope top straight down by `len`, growing as the sim extends it.

use bevy::prelude::*;
use zr_sim::ObjectId;

use crate::convert;
use crate::sim_view::SimRes;

/// Rope thickness in world units (thin; the sim does not model a width).
const ROPE_RADIUS: f32 = 0.35;

/// A rendered rope of a Chinook's drop (`chinook`, rope index).
#[derive(Component)]
struct ChinookRopeView {
    chinook: ObjectId,
    index: usize,
}

/// The shared rope mesh (unit cylinder along +Y, scaled per rope).
#[derive(Resource, Default)]
struct RopeAssets {
    mesh: Option<Handle<Mesh>>,
    material: Option<Handle<StandardMaterial>>,
}

pub fn add_systems(app: &mut App) {
    app.init_resource::<RopeAssets>();
    app.add_systems(
        Update,
        sync_ropes
            .after(crate::sim_view::sim_step)
            .run_if(resource_exists::<crate::sim_view::SimActive>),
    );
}

/// The model condition a descending rappeller should play, from the sim's rappel state. The sim
/// tracks the descent (`RappelState`) but its `condition::derive` does not set `RAPPELLING`, so the
/// client adds it here (`AIRappelState` sets `MODELCONDITION_RAPPELLING` in the engine).
pub fn rappelling(sim: &SimRes, id: ObjectId) -> bool {
    sim.game.logic().rappel_state(id).is_some()
}

#[allow(clippy::too_many_arguments)]
fn sync_ropes(
    mut commands: Commands,
    sim: NonSend<SimRes>,
    mut assets: ResMut<RopeAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    existing: Query<(Entity, &ChinookRopeView)>,
) {
    // The rope mesh/material are built once.
    let mesh = assets.mesh.get_or_insert_with(|| meshes.add(Cylinder::new(ROPE_RADIUS, 1.0))).clone();
    let material = assets
        .material
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::srgb(0.09, 0.07, 0.05),
                unlit: true,
                perceptual_roughness: 1.0,
                ..default()
            })
        })
        .clone();

    // Desired ropes this frame: (chinook, index) -> (start, len).
    let mut desired: Vec<(ObjectId, usize, Vec3, f32)> = Vec::new();
    for o in sim.game.objects() {
        let Some(st) = sim.game.logic().chinook_state(o.id) else { continue };
        for (i, rope) in st.ropes.iter().enumerate() {
            if rope.len <= 0.1 {
                continue;
            }
            desired.push((o.id, i, convert::to_bevy([rope.start.x, rope.start.y, rope.start.z]), rope.len));
        }
    }

    let mut have = std::collections::HashSet::new();
    for (e, view) in &existing {
        let Some((_, _, start, len)) = desired.iter().find(|(id, i, _, _)| *id == view.chinook && *i == view.index) else {
            commands.entity(e).despawn();
            continue;
        };
        have.insert((view.chinook, view.index));
        set_rope(&mut commands, e, *start, *len);
    }
    for (id, index, start, len) in desired {
        if have.contains(&(id, index)) {
            continue;
        }
        let e = commands
            .spawn((ChinookRopeView { chinook: id, index }, Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()), Transform::IDENTITY, Visibility::default(), bevy::light::NotShadowCaster))
            .id();
        set_rope(&mut commands, e, start, len);
    }
}

/// Places a rope cylinder from `start` straight down by `len` (Generals z is up = Bevy +Y).
fn set_rope(commands: &mut Commands, e: Entity, start: Vec3, len: f32) {
    let len = len.max(0.1);
    let mid = start + Vec3::NEG_Y * (len * 0.5);
    commands.entity(e).insert(Transform::from_translation(mid).with_scale(Vec3::new(1.0, len, 1.0)));
}
