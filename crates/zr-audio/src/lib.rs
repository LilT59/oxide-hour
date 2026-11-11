//! Zero Hour audio lookup: the `AudioEvent` / `MusicTrack` / `DialogEvent`
//! definitions, their file resolution and a WAV header reader. No playback.
//!
//! Ported from `Core/GameEngine/Source/Common/INI/INIAudioEventInfo.cpp` (the
//! field table), `Core/GameEngine/Include/Common/AudioEventInfo.h`, and
//! `Core/GameEngine/Source/Common/Audio/GameAudio.cpp` /
//! `AudioEventRTS.cpp` (settings, filename prefix and localization).
//!
//! * [`AudioDatabase::from_ini`] parses a merged [`zr_ini::IniDb`] into typed
//!   events, seeding each from its `Default*` block.
//! * [`AudioDatabase::resolve_reference`] reproduces
//!   `AudioEventRTS::generateFilename` + `adjustForLocalization`, checking the
//!   `[Data\Audio\<folder>\<language>]` path first and the base folder second.
//! * [`AudioEventInfo::choose_index`] is the deterministic variant pick (client
//!   RNG supplied by the caller; the sim RNG is never used).
//! * [`wav::parse_wav`] reads PCM / IMA ADPCM / extensible WAV headers, and
//!   [`wav::decode_ima_adpcm`] / [`wav::to_pcm_wav`] turn IMA blocks into PCM a player can decode.
//!
//! Parsers take untrusted bytes: reads are bounds-checked and malformed input is
//! an error, never a panic.

mod database;
mod event;
pub mod report;
mod settings;
mod value;
pub mod wav;

pub use database::{kind_of, AudioDatabase, AudioReference, ResolvedRef};
pub use event::{control, sound_type, AudioEventInfo, AudioPriority, AudioType};
pub use report::{make_report, AudioReport, KindRefs, Unresolved, WavBreakdown};
pub use settings::AudioSettings;
pub use wav::{parse_wav, WavError, WavFormat, WavInfo};
