//! Zero Hour client-side effect data: `FXList` and `ParticleSystem` templates.
//!
//! Parsed from the merged INI database. `FXList` nuggets follow
//! `GameClient/FXList.cpp` (`TheFXListFieldParse`), particle systems follow
//! `GameClient/System/ParticleSys.cpp` (`ParticleSystemTemplate::m_fieldParseTable`).

mod fxlist;
mod particle;
mod random;

pub use fxlist::{FxDatabase, FxListTemplate, FxNugget};
pub use particle::{
    AlphaKeyframe, Color, ColorKeyframe, EmissionVelocity, EmissionVelocityType, EmissionVolume,
    EmissionVolumeType, ParticlePriority, ParticleShaderType, ParticleSystemTemplate, ParticleType,
    MAX_KEYFRAMES, NUM_PARTICLE_PRIORITIES,
};
pub use random::{ClientRng, Distribution, RandomValue};
