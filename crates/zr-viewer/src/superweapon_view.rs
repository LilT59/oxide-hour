//! Client visuals for the superweapon events the sim emits that are not objects/projectiles by
//! themselves: the particle uplink cannon's orbital laser beam (`LaserUpdate` + `W3DLaserDraw`,
//! driven by `ParticleUplinkCannonUpdate`).
//!
//! The sim owns the firing state (`Object::superweapon.puc`: `current_target`, `laser_status`,
//! `width_scalar`). The engine's `ParticleUplinkCannonUpdate::update` re-points a `W3DLaserDraw`
//! beam from `m_currentTargetPosition + 500` down to `m_currentTargetPosition` every frame
//! (`ParticleUplinkCannonUpdate.cpp:604-616`), so the viewer draws exactly that vertical beam and
//! reads the draw data (`OuterBeamWidth`/`InnerBeamWidth`/`InnerColor`/`OuterColor`/`Texture`) from
//! the `ParticleUplinkCannon_OrbitalLaser` object's `W3DLaserDraw` module.
//!
//! The Spectre gunship, its gattling gunner, the howitzer shells and the neutron missile are real
//! sim objects/projectiles, so `sim_view`/`projectile_sync`/`fx` already draw them; the spy-vision
//! reveal is applied in the sim's shroud (`GameLogic::update_shroud`) and shown by `shroud.rs`.

use std::collections::HashMap;

use bevy::prelude::*;
use zr_sim::ObjectId;

use crate::convert;
use crate::sim_view::{ModelCache, SimRes};

/// The orbital beam's height above the target (`orbitPosition.z += 500.0`).
const ORBIT_HEIGHT: f32 = 500.0;

/// `W3DLaserDrawModuleData` fields the viewer needs.
#[derive(Clone)]
struct LaserDraw {
    texture: String,
    inner_width: f32,
    outer_width: f32,
    inner_color: [f32; 3],
    outer_color: [f32; 3],
}

impl Default for LaserDraw {
    fn default() -> Self {
        // `ParticleUplinkCannon_OrbitalLaser` shipped values.
        LaserDraw { texture: "EXNoise02.tga".into(), inner_width: 0.6, outer_width: 26.0, inner_color: [1.0, 1.0, 1.0], outer_color: [0.0, 0.0, 1.0] }
    }
}

/// One rendered laser cylinder (`core` = the thin white inner beam).
#[derive(Component)]
struct PucBeamView {
    object: ObjectId,
    core: bool,
}

#[derive(Resource, Default)]
struct SwAssets {
    mesh: Option<Handle<Mesh>>,
    laser: Option<LaserDraw>,
    /// `(texture, core)` -> material.
    materials: HashMap<(String, bool), Handle<StandardMaterial>>,
}

pub fn add_systems(app: &mut App) {
    app.init_resource::<SwAssets>();
    app.add_systems(
        Update,
        sync_puc_beams
            .after(crate::sim_view::sim_step)
            .run_if(resource_exists::<crate::sim_view::SimActive>),
    );
}

#[allow(clippy::too_many_arguments)]
fn sync_puc_beams(
    mut commands: Commands,
    sim: NonSend<SimRes>,
    cache: Res<ModelCache>,
    mut assets: ResMut<SwAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    existing: Query<(Entity, &PucBeamView)>,
) {
    let laser = assets.laser.get_or_insert_with(|| laser_draw(&sim).unwrap_or_default()).clone();
    let mesh = assets.mesh.get_or_insert_with(|| meshes.add(Cylinder::new(1.0, 1.0))).clone();

    // Desired beams: for every firing PUC, its outer beam and the white core.
    let mut desired: Vec<(ObjectId, bool, Vec3, f32)> = Vec::new();
    for o in sim.game.logic().objects() {
        let Some(puc) = o.superweapon.puc.as_deref() else { continue };
        // `LASERSTATUS_BORN` / `LASERSTATUS_DECAYING` with a non-zero width are the visible frames.
        if !matches!(puc.laser_status, 1 | 2) || puc.width_scalar <= 0.01 {
            continue;
        }
        let t = puc.current_target;
        let mid = convert::to_bevy([t.x, t.y, t.z + ORBIT_HEIGHT * 0.5]);
        let outer_r = (laser.outer_width * 0.5 * puc.width_scalar).max(0.3);
        let inner_r = (laser.inner_width * 0.5 * puc.width_scalar).max(0.15);
        desired.push((o.id, false, mid, outer_r));
        desired.push((o.id, true, mid, inner_r));
    }

    let mut have = std::collections::HashSet::new();
    for (e, view) in &existing {
        let Some((_, _, mid, radius)) = desired.iter().find(|(id, core, _, _)| *id == view.object && *core == view.core) else {
            commands.entity(e).despawn();
            continue;
        };
        have.insert((view.object, view.core));
        commands.entity(e).insert(Transform::from_translation(*mid).with_scale(Vec3::new(*radius, ORBIT_HEIGHT, *radius)));
    }
    for (id, core, mid, radius) in desired {
        if have.contains(&(id, core)) {
            continue;
        }
        let key = (laser.texture.clone(), core);
        let material = assets
            .materials
            .entry(key)
            .or_insert_with(|| {
                let tex = zr_w3d::load_texture(&cache.vfs, &laser.texture).ok().map(|t| images.add(convert::texture_image(&t, convert::TextureUse::Laser)));
                let color = if core { laser.inner_color } else { laser.outer_color };
                materials.add(StandardMaterial {
                    base_color: Color::srgba(color[0], color[1], color[2], if core { 0.95 } else { 0.7 }),
                    base_color_texture: tex,
                    unlit: true,
                    alpha_mode: AlphaMode::Add,
                    cull_mode: None,
                    double_sided: true,
                    ..default()
                })
            })
            .clone();
        commands.spawn((
            PucBeamView { object: id, core },
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material),
            Transform::from_translation(mid).with_scale(Vec3::new(radius, ORBIT_HEIGHT, radius)),
            Visibility::default(),
            bevy::light::NotShadowCaster,
        ));
    }
}

/// Reads the `W3DLaserDraw` module of `ParticleUplinkCannon_OrbitalLaser` from the template DB.
fn laser_draw(sim: &SimRes) -> Option<LaserDraw> {
    let template = sim.game.data().templates().thing("ParticleUplinkCannon_OrbitalLaser")?;
    for m in &template.modules {
        if let zr_templates::ModuleData::Unparsed { module_type, fields, .. } = &m.data {
            if module_type.eq_ignore_ascii_case("W3DLaserDraw") {
                let get = |k: &str| zr_templates::diag::raw_value(fields, k);
                return Some(LaserDraw {
                    texture: get("Texture").unwrap_or("EXNoise02.tga").to_owned(),
                    inner_width: get("InnerBeamWidth").and_then(|v| v.trim().parse().ok()).unwrap_or(0.6),
                    outer_width: get("OuterBeamWidth").and_then(|v| v.trim().parse().ok()).unwrap_or(26.0),
                    inner_color: get("InnerColor").and_then(parse_color).unwrap_or([1.0, 1.0, 1.0]),
                    outer_color: get("OuterColor").and_then(parse_color).unwrap_or([0.0, 0.0, 1.0]),
                });
            }
        }
    }
    None
}

/// `R:255 G:255 B:255 A:250` -> `[r, g, b]` in 0..1.
fn parse_color(s: &str) -> Option<[f32; 3]> {
    let mut out = [1.0f32; 3];
    for tok in s.split_whitespace() {
        let Some((k, v)) = tok.split_once(':') else { continue };
        let Ok(val) = v.trim().parse::<f32>() else { continue };
        match k.to_ascii_uppercase().as_str() {
            "R" => out[0] = val / 255.0,
            "G" => out[1] = val / 255.0,
            "B" => out[2] = val / 255.0,
            _ => {}
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_laser_color() {
        assert_eq!(parse_color("R:0 G:0 B:255 A:150"), Some([0.0, 0.0, 1.0]));
        assert_eq!(parse_color("R:255 G:255 B:255 A:250"), Some([1.0, 1.0, 1.0]));
    }
}
