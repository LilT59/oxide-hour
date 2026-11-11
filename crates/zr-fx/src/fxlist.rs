//! `FXList` parsing (`GameClient/FXList.cpp`). An `FXList` is an ordered list of
//! nuggets (`TheFXListFieldParse`); each nugget block's fields are parsed by its
//! own field table.

use std::collections::BTreeMap;

use zr_ini::{Block, IniDb};

use crate::particle::{Color, ParticleSystemTemplate};
use crate::random::RandomValue;

/// One `FXList` nugget, in file order.
#[derive(Debug, Clone)]
pub enum FxNugget {
    Sound { name: String },
    RayEffect { name: String, primary_offset: [f32; 3], secondary_offset: [f32; 3] },
    Tracer { tracer_name: String, bone_name: String, speed: f32, decay_at: f32, length: f32, width: f32, color: Color, probability: f32 },
    LightPulse { color: Color, radius: f32, radius_pct: f32, increase_frames: u32, decrease_frames: u32 },
    ViewShake { kind: String },
    TerrainScorch { scorch: String, radius: f32 },
    ParticleSystem {
        name: String,
        count: i32,
        offset: [f32; 3],
        radius: RandomValue,
        height: RandomValue,
        initial_delay: RandomValue,
        rotate: [f32; 3],
        orient_to_object: bool,
        attach_to_object: bool,
        create_at_ground_height: bool,
        use_callers_radius: bool,
        ricochet: bool,
    },
    FxListAtBonePos { fx: String, bone_name: String, orient_to_bone: bool },
}

/// A parsed `FXList NAME ... End`.
#[derive(Debug, Clone)]
pub struct FxListTemplate {
    pub name: String,
    pub nuggets: Vec<FxNugget>,
}

/// Parsed `FXList` and `ParticleSystem` templates, keyed case-insensitively.
#[derive(Debug, Clone, Default)]
pub struct FxDatabase {
    fx_lists: BTreeMap<String, FxListTemplate>,
    particles: BTreeMap<String, ParticleSystemTemplate>,
}

fn coord(b: &Block, key: &str) -> [f32; 3] {
    let Some(v) = b.value(key) else { return [0.0; 3] };
    let mut out = [0.0f32; 3];
    for t in v.split_whitespace() {
        let Some((k, val)) = t.split_once(':') else { continue };
        let idx = match k.to_ascii_uppercase().as_str() {
            "X" => 0,
            "Y" => 1,
            "Z" => 2,
            _ => continue,
        };
        out[idx] = val.trim().parse().unwrap_or(0.0);
    }
    out
}

fn rgb(b: &Block, key: &str) -> Color {
    let Some(v) = b.value(key) else { return Color::default() };
    let mut out = Color::default();
    for t in v.split_whitespace() {
        let Some((k, val)) = t.split_once(':') else { continue };
        let n = val.trim().parse::<f32>().unwrap_or(0.0) / 255.0;
        match k.to_ascii_uppercase().as_str() {
            "R" => out.r = n,
            "G" => out.g = n,
            "B" => out.b = n,
            _ => {}
        }
    }
    out
}

fn real(b: &Block, key: &str, default: f32) -> f32 {
    b.value(key).and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

fn uint(b: &Block, key: &str, default: u32) -> u32 {
    b.value(key).and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

fn boolv(b: &Block, key: &str, default: bool) -> bool {
    match b.value(key).map(|v| v.trim()) {
        Some(v) if v.eq_ignore_ascii_case("yes") || v == "1" || v.eq_ignore_ascii_case("true") => true,
        Some(v) if v.eq_ignore_ascii_case("no") || v == "0" || v.eq_ignore_ascii_case("false") => false,
        _ => default,
    }
}

fn rv(b: &Block, key: &str, default: RandomValue) -> RandomValue {
    b.value(key).and_then(RandomValue::parse).unwrap_or(default)
}

fn nugget(block: &Block) -> Option<FxNugget> {
    Some(match block.kind.as_str() {
        "Sound" => FxNugget::Sound { name: block.value("Name").unwrap_or("").trim().to_owned() },
        "RayEffect" => FxNugget::RayEffect {
            name: block.value("Name").unwrap_or("").trim().to_owned(),
            primary_offset: coord(block, "PrimaryOffset"),
            secondary_offset: coord(block, "SecondaryOffset"),
        },
        "Tracer" => FxNugget::Tracer {
            tracer_name: block.value("TracerName").unwrap_or("GenericTracer").trim().to_owned(),
            bone_name: block.value("BoneName").unwrap_or("").trim().to_owned(),
            speed: real(block, "Speed", 0.0),
            decay_at: real(block, "DecayAt", 1.0),
            length: real(block, "Length", 10.0),
            width: real(block, "Width", 1.0),
            color: rgb(block, "Color"),
            probability: real(block, "Probability", 1.0),
        },
        "LightPulse" => FxNugget::LightPulse {
            color: rgb(block, "Color"),
            radius: real(block, "Radius", 0.0),
            radius_pct: real(block, "RadiusAsPercentOfObjectSize", 0.0),
            increase_frames: uint(block, "IncreaseTime", 0),
            decrease_frames: uint(block, "DecreaseTime", 0),
        },
        "ViewShake" => FxNugget::ViewShake { kind: block.value("Type").unwrap_or("NORMAL").trim().to_ascii_uppercase() },
        "TerrainScorch" => FxNugget::TerrainScorch {
            scorch: block.value("Type").unwrap_or("RANDOM").trim().to_ascii_uppercase(),
            radius: real(block, "Radius", 0.0),
        },
        "ParticleSystem" => FxNugget::ParticleSystem {
            name: block.value("Name").unwrap_or("").trim().to_owned(),
            count: block.value("Count").and_then(|v| v.trim().parse().ok()).unwrap_or(1),
            offset: coord(block, "Offset"),
            radius: rv(block, "Radius", RandomValue::constant(0.0)),
            height: rv(block, "Height", RandomValue::constant(0.0)),
            initial_delay: rv(block, "InitialDelay", RandomValue::constant(-1.0)),
            rotate: [real(block, "RotateX", 0.0), real(block, "RotateY", 0.0), real(block, "RotateZ", 0.0)],
            orient_to_object: boolv(block, "OrientToObject", false),
            attach_to_object: boolv(block, "AttachToObject", false),
            create_at_ground_height: boolv(block, "CreateAtGroundHeight", false),
            use_callers_radius: boolv(block, "UseCallersRadius", false),
            ricochet: boolv(block, "Ricochet", false),
        },
        "FXListAtBonePos" => FxNugget::FxListAtBonePos {
            fx: block.value("FX").unwrap_or("").trim().to_owned(),
            bone_name: block.value("BoneName").unwrap_or("").trim().to_owned(),
            orient_to_bone: boolv(block, "OrientToBone", true),
        },
        _ => return None,
    })
}

impl FxDatabase {
    /// Parses every `FXList` and `ParticleSystem` block in the database.
    pub fn from_ini(ini: &IniDb) -> Self {
        let mut db = FxDatabase::default();
        for b in ini.of_kind("ParticleSystem") {
            if let Some(t) = ParticleSystemTemplate::parse(b) {
                db.particles.insert(t.name.to_ascii_lowercase(), t);
            }
        }
        for b in ini.of_kind("FXList") {
            let Some(name) = b.name() else { continue };
            let nuggets = b.children().filter_map(nugget).collect();
            db.fx_lists.insert(name.to_ascii_lowercase(), FxListTemplate { name: name.to_owned(), nuggets });
        }
        db
    }

    pub fn fx_list(&self, name: &str) -> Option<&FxListTemplate> {
        self.fx_lists.get(&name.to_ascii_lowercase())
    }

    pub fn particle(&self, name: &str) -> Option<&ParticleSystemTemplate> {
        self.particles.get(&name.to_ascii_lowercase())
    }

    pub fn fx_list_count(&self) -> usize {
        self.fx_lists.len()
    }

    pub fn particle_count(&self) -> usize {
        self.particles.len()
    }

    pub fn fx_lists(&self) -> impl Iterator<Item = &FxListTemplate> {
        self.fx_lists.values()
    }

    pub fn particles(&self) -> impl Iterator<Item = &ParticleSystemTemplate> {
        self.particles.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(text: &str) -> FxDatabase {
        let mut ini = IniDb::new();
        ini.extend(zr_ini::parse("t.ini", text).unwrap());
        FxDatabase::from_ini(&ini)
    }

    #[test]
    fn parses_fxlist_nuggets_in_order() {
        let db = db("FXList FX_Test\n  ParticleSystem\n    Name = Smoke\n    Offset = X:0 Y:0 Z:2\n    Count = 2\n  End\n  ViewShake\n    Type = STRONG\n  End\n  Sound\n    Name = TankDie\n  End\nEnd\nParticleSystem Smoke\n  Shader = ADDITIVE\nEnd\n");
        let fx = db.fx_list("fx_test").unwrap();
        assert_eq!(fx.nuggets.len(), 3);
        match &fx.nuggets[0] {
            FxNugget::ParticleSystem { name, count, offset, .. } => {
                assert_eq!(name, "Smoke");
                assert_eq!(*count, 2);
                assert_eq!(*offset, [0.0, 0.0, 2.0]);
            }
            _ => panic!("expected particle system"),
        }
        assert!(matches!(&fx.nuggets[1], FxNugget::ViewShake { kind } if kind == "STRONG"));
        assert!(matches!(&fx.nuggets[2], FxNugget::Sound { name } if name == "TankDie"));
        assert_eq!(db.particle("smoke").unwrap().shader, crate::particle::ParticleShaderType::Additive);
    }
}
