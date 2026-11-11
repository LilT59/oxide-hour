//! `ParticleSystemTemplate` parsing (`GameClient/System/ParticleSys.cpp`,
//! `ParticleSystemTemplate::m_fieldParseTable`; defaults from
//! `ParticleSystemInfo::ParticleSystemInfo`).

use zr_ini::Block;

use crate::random::RandomValue;

/// `ParticleSystemInfo::ParticleShaderType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParticleShaderType {
    Additive,
    #[default]
    Alpha,
    AlphaTest,
    Multiply,
}

impl ParticleShaderType {
    fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_uppercase().as_str() {
            "ADDITIVE" => Self::Additive,
            "ALPHA" => Self::Alpha,
            "ALPHA_TEST" => Self::AlphaTest,
            "MULTIPLY" => Self::Multiply,
            _ => return None,
        })
    }
}

/// `ParticleSystemInfo::ParticleType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParticleType {
    #[default]
    Particle,
    Drawable,
    Streak,
    VolumeParticle,
    Smudge,
}

impl ParticleType {
    fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_uppercase().as_str() {
            "PARTICLE" => Self::Particle,
            "DRAWABLE" => Self::Drawable,
            "STREAK" => Self::Streak,
            "VOLUME_PARTICLE" => Self::VolumeParticle,
            "SMUDGE" => Self::Smudge,
            _ => return None,
        })
    }
}

/// `ParticlePriorityType` (`GameClient/ParticleSys.h:87-110`). The values are the indices of
/// `ParticlePriorityNames` (`ParticleSys.h:504-507`), which `INI::parseIndexList` parses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
#[repr(u8)]
pub enum ParticlePriority {
    /// `INVALID_PRIORITY` (0). Never set from an INI block.
    Invalid = 0,
    /// `PARTICLE_PRIORITY_LOWEST` (1) and the `ParticleSystemInfo` default (`ParticleSys.cpp:845`).
    #[default]
    WeaponExplosion = 1,
    Scorchmark,
    DustTrail,
    Buildup,
    DebrisTrail,
    UnitDamageFx,
    DeathExplosion,
    SemiConstant,
    Constant,
    WeaponTrail,
    AreaEffect,
    Critical,
    /// `ALWAYS_RENDER`: exempt from the particle cap and the LOD gates.
    AlwaysRender,
}

impl ParticlePriority {
    /// `ParticlePriorityNames` (`ParticleSys.h:504-507`), in order. `INI::parseIndexList` matches
    /// these names case-insensitively.
    pub const NAMES: [&'static str; 14] = [
        "NONE",
        "WEAPON_EXPLOSION",
        "SCORCHMARK",
        "DUST_TRAIL",
        "BUILDUP",
        "DEBRIS_TRAIL",
        "UNIT_DAMAGE_FX",
        "DEATH_EXPLOSION",
        "SEMI_CONSTANT",
        "CONSTANT",
        "WEAPON_TRAIL",
        "AREA_EFFECT",
        "CRITICAL",
        "ALWAYS_RENDER",
    ];

    /// Parses a `Priority` token (an unknown token throws `INI_INVALID_DATA` in the engine).
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        Self::NAMES.iter().position(|n| n.eq_ignore_ascii_case(s)).map(Self::from_index)
    }

    /// The index into `ParticlePriorityNames` (`PARTICLE_PRIORITY_LOWEST` is 1).
    pub const fn index(self) -> usize {
        self as usize
    }

    /// From a `ParticlePriorityNames` index (values past `ALWAYS_RENDER` are `Invalid`).
    pub const fn from_index(i: usize) -> Self {
        match i {
            1 => Self::WeaponExplosion,
            2 => Self::Scorchmark,
            3 => Self::DustTrail,
            4 => Self::Buildup,
            5 => Self::DebrisTrail,
            6 => Self::UnitDamageFx,
            7 => Self::DeathExplosion,
            8 => Self::SemiConstant,
            9 => Self::Constant,
            10 => Self::WeaponTrail,
            11 => Self::AreaEffect,
            12 => Self::Critical,
            13 => Self::AlwaysRender,
            _ => Self::Invalid,
        }
    }
}

/// `NUM_PARTICLE_PRIORITIES` (`ParticleSys.h:112`): the length of the manager's per-priority lists.
pub const NUM_PARTICLE_PRIORITIES: usize = 14;

/// `ParticleSystemInfo::EmissionVelocityType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmissionVelocityType {
    #[default]
    None,
    Ortho,
    Spherical,
    Hemispherical,
    Cylindrical,
    Outward,
}

impl EmissionVelocityType {
    fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_uppercase().as_str() {
            "ORTHO" => Self::Ortho,
            "SPHERICAL" => Self::Spherical,
            "HEMISPHERICAL" => Self::Hemispherical,
            "CYLINDRICAL" => Self::Cylindrical,
            "OUTWARD" => Self::Outward,
            _ => return None,
        })
    }
}

/// `ParticleSystemInfo::EmissionVolumeType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmissionVolumeType {
    #[default]
    Point,
    Line,
    Box,
    Sphere,
    Cylinder,
}

impl EmissionVolumeType {
    fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_uppercase().as_str() {
            "POINT" => Self::Point,
            "LINE" => Self::Line,
            "BOX" => Self::Box,
            "SPHERE" => Self::Sphere,
            "CYLINDER" => Self::Cylinder,
            _ => return None,
        })
    }
}

/// One emission velocity shape (the C++ union).
#[derive(Debug, Clone, Default)]
pub enum EmissionVelocity {
    #[default]
    None,
    Ortho { x: RandomValue, y: RandomValue, z: RandomValue },
    Spherical { speed: RandomValue },
    Hemispherical { speed: RandomValue },
    Cylindrical { radial: RandomValue, normal: RandomValue },
    Outward { speed: RandomValue, other: RandomValue },
}

/// One emission volume shape (the C++ union).
#[derive(Debug, Clone, Default)]
pub enum EmissionVolume {
    #[default]
    Point,
    Line { start: [f32; 3], end: [f32; 3] },
    Box { half: [f32; 3] },
    Sphere { radius: f32 },
    Cylinder { radius: f32, length: f32 },
}

/// An `RGBColor` in 0..1.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

/// `ParticleSystemInfo::RandomKeyframe`: `Alpha1 = low high frame`.
#[derive(Debug, Clone, Copy, Default)]
pub struct AlphaKeyframe {
    pub var: RandomValue,
    pub frame: u32,
}

/// `RGBColorKeyframe`: `Color1 = R:r G:g B:b frame`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ColorKeyframe {
    pub color: Color,
    pub frame: u32,
}

/// `ParticleSystemInfo::MAX_KEYFRAMES`.
pub const MAX_KEYFRAMES: usize = 8;

/// A parsed `ParticleSystem` template.
#[derive(Debug, Clone)]
pub struct ParticleSystemTemplate {
    pub name: String,
    /// `Priority` (`INI::parseIndexList`, `ParticleSys.cpp:2887`); absent is `WEAPON_EXPLOSION`
    /// (`ParticleSystemInfo::ParticleSystemInfo`, `ParticleSys.cpp:845`).
    pub priority: ParticlePriority,
    pub is_one_shot: bool,
    pub shader: ParticleShaderType,
    pub particle_type: ParticleType,
    pub particle_name: String,
    pub angle_z: RandomValue,
    pub angular_rate_z: RandomValue,
    pub angular_damping: RandomValue,
    pub velocity_damping: RandomValue,
    pub gravity: f32,
    pub lifetime: RandomValue,
    pub system_lifetime: u32,
    pub start_size: RandomValue,
    pub start_size_rate: RandomValue,
    pub size_rate: RandomValue,
    pub size_rate_damping: RandomValue,
    pub alpha_keys: [AlphaKeyframe; MAX_KEYFRAMES],
    pub color_keys: [ColorKeyframe; MAX_KEYFRAMES],
    /// Raw 0..255 range; the runtime divides by 255 (`ParticleSystem` ctor).
    pub color_scale: RandomValue,
    pub burst_delay: RandomValue,
    pub burst_count: RandomValue,
    pub initial_delay: RandomValue,
    pub drift_velocity: [f32; 3],
    pub velocity_type: EmissionVelocityType,
    pub velocity: EmissionVelocity,
    pub volume_type: EmissionVolumeType,
    pub volume: EmissionVolume,
    pub is_hollow: bool,
    pub ground_aligned: bool,
    pub emit_above_ground_only: bool,
    pub particle_up_towards_emitter: bool,
    pub attached_system: String,
    pub slave_system: String,
    pub slave_pos_offset: [f32; 3],
}

impl Default for ParticleSystemTemplate {
    fn default() -> Self {
        Self {
            name: String::new(),
            priority: ParticlePriority::WeaponExplosion,
            is_one_shot: false,
            shader: ParticleShaderType::Alpha,
            particle_type: ParticleType::Particle,
            particle_name: String::new(),
            angle_z: RandomValue::default(),
            angular_rate_z: RandomValue::default(),
            angular_damping: RandomValue::constant(1.0),
            velocity_damping: RandomValue::constant(1.0),
            gravity: 0.0,
            lifetime: RandomValue::constant(0.0),
            system_lifetime: 0,
            start_size: RandomValue::default(),
            start_size_rate: RandomValue::default(),
            size_rate: RandomValue::default(),
            size_rate_damping: RandomValue::constant(1.0),
            alpha_keys: [AlphaKeyframe::default(); MAX_KEYFRAMES],
            color_keys: [ColorKeyframe::default(); MAX_KEYFRAMES],
            color_scale: RandomValue::default(),
            burst_delay: RandomValue::default(),
            burst_count: RandomValue::default(),
            initial_delay: RandomValue::default(),
            drift_velocity: [0.0; 3],
            velocity_type: EmissionVelocityType::None,
            velocity: EmissionVelocity::None,
            volume_type: EmissionVolumeType::Point,
            volume: EmissionVolume::Point,
            is_hollow: false,
            ground_aligned: false,
            emit_above_ground_only: false,
            particle_up_towards_emitter: false,
            attached_system: String::new(),
            slave_system: String::new(),
            slave_pos_offset: [0.0; 3],
        }
    }
}

fn rv(b: &Block, key: &str) -> Option<RandomValue> {
    b.value(key).and_then(RandomValue::parse)
}

fn real(b: &Block, key: &str, default: f32) -> f32 {
    b.value(key).and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

fn uint(b: &Block, key: &str, default: u32) -> u32 {
    b.value(key).and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

fn flag(b: &Block, key: &str) -> bool {
    b.value(key).is_some_and(|v| v.trim().eq_ignore_ascii_case("yes") || v.trim() == "1" || v.trim().eq_ignore_ascii_case("true"))
}

/// `X:1 Y:2 Z:3` (missing components default to 0).
fn coord(b: &Block, key: &str) -> Option<[f32; 3]> {
    let v = b.value(key)?;
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
    Some(out)
}

/// `R:1 G:2 B:3` (missing components default to 0).
fn rgb(b: &Block, key: &str) -> Option<Color> {
    let v = b.value(key)?;
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
    Some(out)
}

impl ParticleSystemTemplate {
    /// Parses one `ParticleSystem NAME ... End` block.
    pub fn parse(block: &Block) -> Option<Self> {
        let mut t = ParticleSystemTemplate { name: block.name()?.to_owned(), ..Default::default() };
        // A `Priority` the engine's name list does not know throws `INI_INVALID_DATA` out of
        // `parseIndexList`, so the template is not created.
        if let Some(v) = block.value("Priority") {
            t.priority = ParticlePriority::parse(v)?;
        }
        t.is_one_shot = flag(block, "IsOneShot");
        if let Some(s) = block.value("Shader").and_then(ParticleShaderType::parse) {
            t.shader = s;
        }
        if let Some(s) = block.value("Type").and_then(ParticleType::parse) {
            t.particle_type = s;
        }
        t.particle_name = block.value("ParticleName").unwrap_or("").trim().to_owned();
        t.angle_z = rv(block, "AngleZ").unwrap_or_default();
        t.angular_rate_z = rv(block, "AngularRateZ").unwrap_or_default();
        t.angular_damping = rv(block, "AngularDamping").unwrap_or_else(|| RandomValue::constant(1.0));
        t.velocity_damping = rv(block, "VelocityDamping").unwrap_or_else(|| RandomValue::constant(1.0));
        t.gravity = real(block, "Gravity", 0.0);
        t.lifetime = rv(block, "Lifetime").unwrap_or_default();
        t.system_lifetime = uint(block, "SystemLifetime", 0);
        t.start_size = rv(block, "Size").unwrap_or_default();
        t.start_size_rate = rv(block, "StartSizeRate").unwrap_or_default();
        t.size_rate = rv(block, "SizeRate").unwrap_or_default();
        t.size_rate_damping = rv(block, "SizeRateDamping").unwrap_or_else(|| RandomValue::constant(1.0));
        for i in 0..MAX_KEYFRAMES {
            if let Some(v) = block.value(&format!("Alpha{}", i + 1)) {
                let toks: Vec<&str> = v.split_whitespace().collect();
                let var = toks.first().zip(toks.get(1)).and_then(|(a, b)| RandomValue::parse(&format!("{a} {b}"))).unwrap_or_default();
                let frame = toks.get(2).and_then(|t| t.parse().ok()).unwrap_or(0);
                t.alpha_keys[i] = AlphaKeyframe { var, frame };
            }
            if let Some(v) = block.value(&format!("Color{}", i + 1)) {
                let color = rgb(block, &format!("Color{}", i + 1)).unwrap_or_default();
                let frame = v.split_whitespace().last().and_then(|t| t.parse().ok()).unwrap_or(0);
                t.color_keys[i] = ColorKeyframe { color, frame };
            }
        }
        t.color_scale = rv(block, "ColorScale").unwrap_or_default();
        t.burst_delay = rv(block, "BurstDelay").unwrap_or_default();
        t.burst_count = rv(block, "BurstCount").unwrap_or_default();
        t.initial_delay = rv(block, "InitialDelay").unwrap_or_default();
        t.drift_velocity = coord(block, "DriftVelocity").unwrap_or([0.0; 3]);
        t.slave_system = block.value("SlaveSystem").unwrap_or("").trim().to_owned();
        t.slave_pos_offset = coord(block, "SlavePosOffset").unwrap_or([0.0; 3]);
        t.attached_system = block.value("PerParticleAttachedSystem").unwrap_or("").trim().to_owned();

        t.velocity_type = block.value("VelocityType").and_then(EmissionVelocityType::parse).unwrap_or_default();
        t.velocity = match t.velocity_type {
            EmissionVelocityType::Ortho => EmissionVelocity::Ortho {
                x: rv(block, "VelOrthoX").unwrap_or_default(),
                y: rv(block, "VelOrthoY").unwrap_or_default(),
                z: rv(block, "VelOrthoZ").unwrap_or_default(),
            },
            EmissionVelocityType::Spherical => EmissionVelocity::Spherical { speed: rv(block, "VelSpherical").unwrap_or_default() },
            EmissionVelocityType::Hemispherical => EmissionVelocity::Hemispherical { speed: rv(block, "VelHemispherical").unwrap_or_default() },
            EmissionVelocityType::Cylindrical => EmissionVelocity::Cylindrical {
                radial: rv(block, "VelCylindricalRadial").unwrap_or_default(),
                normal: rv(block, "VelCylindricalNormal").unwrap_or_default(),
            },
            EmissionVelocityType::Outward => EmissionVelocity::Outward {
                speed: rv(block, "VelOutward").unwrap_or_default(),
                other: rv(block, "VelOutwardOther").unwrap_or_default(),
            },
            EmissionVelocityType::None => EmissionVelocity::None,
        };

        t.volume_type = block.value("VolumeType").and_then(EmissionVolumeType::parse).unwrap_or_default();
        t.volume = match t.volume_type {
            EmissionVolumeType::Point => EmissionVolume::Point,
            EmissionVolumeType::Line => EmissionVolume::Line {
                start: coord(block, "VolLineStart").unwrap_or([0.0; 3]),
                end: coord(block, "VolLineEnd").unwrap_or([0.0; 3]),
            },
            EmissionVolumeType::Box => EmissionVolume::Box { half: coord(block, "VolBoxHalfSize").unwrap_or([0.0; 3]) },
            EmissionVolumeType::Sphere => EmissionVolume::Sphere { radius: real(block, "VolSphereRadius", 0.0) },
            EmissionVolumeType::Cylinder => EmissionVolume::Cylinder {
                radius: real(block, "VolCylinderRadius", 0.0),
                length: real(block, "VolCylinderLength", 0.0),
            },
        };
        t.is_hollow = flag(block, "IsHollow");
        t.ground_aligned = block.value("IsGroundAligned").is_some_and(|v| v.trim().eq_ignore_ascii_case("yes"));
        t.emit_above_ground_only = flag(block, "IsEmitAboveGroundOnly");
        t.particle_up_towards_emitter = flag(block, "IsParticleUpTowardsEmitter");
        Some(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_one(text: &str) -> ParticleSystemTemplate {
        let mut db = zr_ini::IniDb::new();
        db.extend(zr_ini::parse("t.ini", text).unwrap());
        let b = db.get("ParticleSystem", "PS").unwrap();
        ParticleSystemTemplate::parse(b).unwrap()
    }

    #[test]
    fn parses_smoke() {
        let t = parse_one(
            "ParticleSystem PS\n  Shader = ALPHA\n  Type = PARTICLE\n  ParticleName = EXSmokePuf07.tga\n  Size = 1.00 2.00\n  SizeRate = 3.00 3.00\n  SizeRateDamping = 0.80 0.80\n  VelocityDamping = 0.99 0.98\n  Gravity = 0.01\n  Lifetime = 50.00 50.00\n  SystemLifetime = 10\n  Alpha1 = 0.00 0.00 0\n  Alpha2 = 0.50 0.50 10\n  Alpha3 = 0.00 0.00 50\n  Color1 = R:255 G:134 B:0 0\n  Color2 = R:0 G:0 B:0 0\n  BurstDelay = 40.00 40.00\n  BurstCount = 0.00 2.00\n  DriftVelocity = X:0.04 Y:0.08 Z:0.10\n  VelocityType = OUTWARD\n  VelOutward = 10.00 10.00\n  VolumeType = CYLINDER\n  VolCylinderRadius = 10.00\n  VolCylinderLength = 0.00\n  IsHollow = No\n  IsGroundAligned = No\nEnd\n",
        );
        assert_eq!(t.name, "PS");
        assert_eq!(t.shader, ParticleShaderType::Alpha);
        assert_eq!(t.particle_name, "EXSmokePuf07.tga");
        assert_eq!(t.lifetime.low, 50.0);
        assert_eq!(t.system_lifetime, 10);
        assert_eq!(t.alpha_keys[1].frame, 10);
        assert!((t.alpha_keys[1].var.low - 0.5).abs() < 1e-6);
        assert!((t.color_keys[0].color.r - 1.0).abs() < 1e-6);
        assert_eq!(t.drift_velocity, [0.04, 0.08, 0.10]);
        assert_eq!(t.velocity_type, EmissionVelocityType::Outward);
        assert_eq!(t.volume_type, EmissionVolumeType::Cylinder);
        assert!(!t.is_hollow);
    }

    #[test]
    fn defaults() {
        let t = parse_one("ParticleSystem PS\nEnd\n");
        assert_eq!(t.shader, ParticleShaderType::Alpha);
        assert_eq!(t.volume_type, EmissionVolumeType::Point);
        assert_eq!(t.system_lifetime, 0);
        assert_eq!(t.priority, ParticlePriority::WeaponExplosion, "ParticleSystemInfo's default");
    }

    /// `ParticlePriorityNames` (`ParticleSys.h:504-507`) parse by name, any case, to the engine's
    /// `ParticlePriorityType` values. Reverting `Priority` to a raw string (or ignoring the token)
    /// fails this test.
    #[test]
    fn priority_names_parse_to_the_engine_values() {
        let want = [
            ("WEAPON_EXPLOSION", ParticlePriority::WeaponExplosion),
            ("SCORCHMARK", ParticlePriority::Scorchmark),
            ("DUST_TRAIL", ParticlePriority::DustTrail),
            ("BUILDUP", ParticlePriority::Buildup),
            ("DEBRIS_TRAIL", ParticlePriority::DebrisTrail),
            ("UNIT_DAMAGE_FX", ParticlePriority::UnitDamageFx),
            ("DEATH_EXPLOSION", ParticlePriority::DeathExplosion),
            ("SEMI_CONSTANT", ParticlePriority::SemiConstant),
            ("CONSTANT", ParticlePriority::Constant),
            ("WEAPON_TRAIL", ParticlePriority::WeaponTrail),
            ("AREA_EFFECT", ParticlePriority::AreaEffect),
            ("CRITICAL", ParticlePriority::Critical),
            ("ALWAYS_RENDER", ParticlePriority::AlwaysRender),
        ];
        for (name, prio) in want {
            let t = parse_one(&format!("ParticleSystem PS\n Priority = {name}\nEnd\n"));
            assert_eq!(t.priority, prio, "{name}");
            assert_eq!(ParticlePriority::parse(&name.to_ascii_lowercase()), Some(prio), "{name} (lowercase)");
        }
        // `NONE` is `INVALID_PRIORITY` 0, below `PARTICLE_PRIORITY_LOWEST`.
        assert_eq!(ParticlePriority::parse("NONE"), Some(ParticlePriority::Invalid));
        assert_eq!(ParticlePriority::parse("BOGUS"), None);
        assert!(ParticlePriority::WeaponExplosion < ParticlePriority::AlwaysRender);
    }

    /// An unknown `Priority` fails the template in the engine (`INI::parseIndexList` throws
    /// `INI_INVALID_DATA`), so no `ParticleSystemTemplate` is built. Reverting the parse to
    /// `unwrap_or_default` fails this test.
    #[test]
    fn an_unknown_priority_drops_the_template() {
        let mut db = zr_ini::IniDb::new();
        db.extend(zr_ini::parse("t.ini", "ParticleSystem PS\n Priority = BOGUS\nEnd\n").unwrap());
        assert!(ParticleSystemTemplate::parse(db.get("ParticleSystem", "PS").unwrap()).is_none());
    }
}
