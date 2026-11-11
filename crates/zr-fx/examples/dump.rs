//! Parses every `FXList` and `ParticleSystem` in the Zero Hour install and
//! reports counts, distributions and dangling references.
//!
//! Usage: `cargo run --release -p zr-fx --example dump`

use std::collections::BTreeMap;
use zr_assets::{Installs, Vfs};
use zr_fx::{FxDatabase, FxNugget, ParticleShaderType, ParticleType, EmissionVelocityType, EmissionVolumeType};

fn main() {
    let install = Installs::detect().zero_hour.expect("Zero Hour install not found");
    let vfs = Vfs::zero_hour(&install).expect("mounting Zero Hour");
    let ini = zr_ini::load_zero_hour(&vfs).db;

    let db = FxDatabase::from_ini(&ini);
    println!("INI blocks: FXList {} unique, ParticleSystem {} unique", ini.count_unique("FXList"), ini.count_unique("ParticleSystem"));
    println!("parsed: FXList {}, ParticleSystem {}", db.fx_list_count(), db.particle_count());

    let mut shaders: BTreeMap<&str, usize> = BTreeMap::new();
    let mut types: BTreeMap<&str, usize> = BTreeMap::new();
    let mut vel: BTreeMap<&str, usize> = BTreeMap::new();
    let mut vol: BTreeMap<&str, usize> = BTreeMap::new();
    let mut missing_texture = 0;
    let mut texture_ok = 0;
    let mut texture_fail = 0;
    let mut seen_tex = std::collections::BTreeSet::new();
    for p in db.particles() {
        *shaders.entry(shader_name(p.shader)).or_default() += 1;
        *types.entry(type_name(p.particle_type)).or_default() += 1;
        *vel.entry(vel_name(p.velocity_type)).or_default() += 1;
        *vol.entry(vol_name(p.volume_type)).or_default() += 1;
        if p.particle_name.is_empty() {
            missing_texture += 1;
        } else if seen_tex.insert(p.particle_name.to_ascii_lowercase()) {
            match zr_w3d::load_texture(&vfs, &p.particle_name) {
                Ok(_) => texture_ok += 1,
                Err(e) => {
                    texture_fail += 1;
                    if texture_fail <= 10 {
                        println!("  TEXTURE FAIL {}: {e}", p.particle_name);
                    }
                }
            }
        }
    }
    println!("shaders: {shaders:?}");
    println!("types: {types:?}");
    println!("velocity: {vel:?}");
    println!("volume: {vol:?}");
    println!("particle textures: {texture_ok} ok, {texture_fail} fail, {missing_texture} empty");

    let mut nugget_kinds: BTreeMap<&str, usize> = BTreeMap::new();
    let mut dangling = 0;
    for fx in db.fx_lists() {
        for n in &fx.nuggets {
            *nugget_kinds.entry(nugget_name(n)).or_default() += 1;
            if let FxNugget::ParticleSystem { name, .. } = n {
                if db.particle(name).is_none() {
                    dangling += 1;
                    if dangling <= 10 {
                        println!("  DANGLING {} -> {name}", fx.name);
                    }
                }
            }
        }
    }
    println!("FXList nuggets: {nugget_kinds:?}");
    println!("dangling ParticleSystem refs: {dangling}");

    // A couple of known FXLists.
    for name in ["FX_BattleMasterExplosionOneFinal", "FX_GenericTankExplosion"] {
        if let Some(fx) = db.fx_list(name) {
            let systems: Vec<&str> = fx.nuggets.iter().filter_map(|n| match n {
                FxNugget::ParticleSystem { name, .. } => Some(name.as_str()),
                _ => None,
            }).collect();
            println!("{name}: {} nuggets, particle systems {systems:?}", fx.nuggets.len());
        } else {
            println!("{name}: not found");
        }
    }
}

fn shader_name(s: ParticleShaderType) -> &'static str {
    match s {
        ParticleShaderType::Additive => "ADDITIVE",
        ParticleShaderType::Alpha => "ALPHA",
        ParticleShaderType::AlphaTest => "ALPHA_TEST",
        ParticleShaderType::Multiply => "MULTIPLY",
    }
}
fn type_name(t: ParticleType) -> &'static str {
    match t {
        ParticleType::Particle => "PARTICLE",
        ParticleType::Drawable => "DRAWABLE",
        ParticleType::Streak => "STREAK",
        ParticleType::VolumeParticle => "VOLUME_PARTICLE",
        ParticleType::Smudge => "SMUDGE",
    }
}
fn vel_name(v: EmissionVelocityType) -> &'static str {
    match v {
        EmissionVelocityType::None => "NONE",
        EmissionVelocityType::Ortho => "ORTHO",
        EmissionVelocityType::Spherical => "SPHERICAL",
        EmissionVelocityType::Hemispherical => "HEMISPHERICAL",
        EmissionVelocityType::Cylindrical => "CYLINDRICAL",
        EmissionVelocityType::Outward => "OUTWARD",
    }
}
fn vol_name(v: EmissionVolumeType) -> &'static str {
    match v {
        EmissionVolumeType::Point => "POINT",
        EmissionVolumeType::Line => "LINE",
        EmissionVolumeType::Box => "BOX",
        EmissionVolumeType::Sphere => "SPHERE",
        EmissionVolumeType::Cylinder => "CYLINDER",
    }
}
fn nugget_name(n: &FxNugget) -> &'static str {
    match n {
        FxNugget::Sound { .. } => "Sound",
        FxNugget::RayEffect { .. } => "RayEffect",
        FxNugget::Tracer { .. } => "Tracer",
        FxNugget::LightPulse { .. } => "LightPulse",
        FxNugget::ViewShake { .. } => "ViewShake",
        FxNugget::TerrainScorch { .. } => "TerrainScorch",
        FxNugget::ParticleSystem { .. } => "ParticleSystem",
        FxNugget::FxListAtBonePos { .. } => "FXListAtBonePos",
    }
}
