//! `AudioSettings` (`Core/GameEngine/Source/Common/Audio/GameAudio.cpp`,
//! `audioSettingsFieldParseTable` in `AudioManager::init`) plus the path helpers
//! the filename lookup uses (`AudioEventRTS::generateFilenamePrefix` /
//! `generateFilenameExtension`, `Core/GameEngine/Source/Common/Audio/AudioEventRTS.cpp`).
//!
//! Retail ships exactly one `AudioSettings` block, in `Data\INI\AudioSettings.ini`.

use zr_ini::{Block, IniDb};

use crate::{value, AudioType};

/// Engine defaults from the C++ `AudioSettings` constructor / `AudioEventRTS`
/// copies. Paths are empty until the INI is parsed (retail always ships values).
#[derive(Debug, Clone, PartialEq)]
pub struct AudioSettings {
    pub audio_root: String,
    pub sounds_folder: String,
    pub music_folder: String,
    pub streaming_folder: String,
    pub sounds_extension: String,

    pub use_digital: bool,
    pub use_midi: bool,
    pub output_rate: i32,
    pub output_bits: i32,
    pub output_channels: i32,
    pub sample_count_2d: i32,
    pub sample_count_3d: i32,
    pub stream_count: i32,
    /// `Preferred3DHW1..4` then `Preferred3DSW`, in table order.
    pub preferred_3d_providers: Vec<String>,
    pub default_2d_speaker_type: String,
    pub default_3d_speaker_type: String,

    pub min_volume: f32,
    pub use_3d_sound_range_volume_fade: bool,
    pub sound_range_volume_fade_exponent: f32,
    pub global_min_range: i32,
    pub global_max_range: i32,
    /// `parseDurationUnsignedInt`: milliseconds rounded up to 30 Hz frames.
    pub drawable_ambient_frames: u32,
    pub fade_audio_frames: u32,
    pub max_cache_size: u32,

    pub relative_2d_volume: f32,
    pub default_sound_volume: f32,
    pub default_3d_sound_volume: f32,
    pub default_speech_volume: f32,
    pub default_music_volume: f32,
    pub default_money_transaction_volume: f32,

    pub microphone_desired_height_above_terrain: f32,
    pub microphone_max_percentage_between_ground_and_camera: f32,
    pub zoom_min_distance: f32,
    pub zoom_max_distance: f32,
    pub zoom_sound_volume_percentage_amount: f32,
}

impl Default for AudioSettings {
    fn default() -> Self {
        AudioSettings {
            audio_root: String::new(),
            sounds_folder: String::new(),
            music_folder: String::new(),
            streaming_folder: String::new(),
            sounds_extension: "wav".to_owned(),
            use_digital: true,
            use_midi: false,
            output_rate: 0,
            output_bits: 0,
            output_channels: 0,
            sample_count_2d: 0,
            sample_count_3d: 0,
            stream_count: 0,
            preferred_3d_providers: Vec::new(),
            default_2d_speaker_type: String::new(),
            default_3d_speaker_type: String::new(),
            min_volume: 0.0,
            use_3d_sound_range_volume_fade: true,
            sound_range_volume_fade_exponent: 4.0,
            global_min_range: 0,
            global_max_range: 0,
            drawable_ambient_frames: 0,
            fade_audio_frames: 0,
            max_cache_size: 0,
            relative_2d_volume: 0.0,
            default_sound_volume: 0.0,
            default_3d_sound_volume: 0.0,
            default_speech_volume: 0.0,
            default_music_volume: 0.0,
            default_money_transaction_volume: 0.0,
            microphone_desired_height_above_terrain: 0.0,
            microphone_max_percentage_between_ground_and_camera: 0.0,
            zoom_min_distance: 0.0,
            zoom_max_distance: 0.0,
            zoom_sound_volume_percentage_amount: 0.0,
        }
    }
}

impl AudioSettings {
    /// Latest (merged) `AudioSettings` block, or the defaults when absent.
    pub fn from_ini(db: &IniDb) -> AudioSettings {
        let mut s = AudioSettings::default();
        // `resolved` overlays every definition of the unnamed block, last field wins.
        if let Some(b) = db.resolved("AudioSettings", "") {
            s.apply(&b);
        } else if let Some(b) = db.get("AudioSettings", "") {
            s.apply(b);
        }
        s
    }

    /// Applies the fields present in `b`; absent fields keep their current value,
    /// matching the engine's copy-then-`initFromINI` behaviour.
    pub fn apply(&mut self, b: &Block) {
        let set_str = |dst: &mut String, key: &str| {
            if let Some(v) = b.value(key) {
                *dst = value::parse_string(v);
            }
        };
        set_str(&mut self.audio_root, "AudioRoot");
        set_str(&mut self.sounds_folder, "SoundsFolder");
        set_str(&mut self.music_folder, "MusicFolder");
        set_str(&mut self.streaming_folder, "StreamingFolder");
        set_str(&mut self.sounds_extension, "SoundsExtension");
        if let Some(v) = b.value("UseDigital") {
            self.use_digital = value::parse_bool(v);
        }
        if let Some(v) = b.value("UseMidi") {
            self.use_midi = value::parse_bool(v);
        }
        let set_int = |dst: &mut i32, key: &str| {
            if let Some(v) = b.value(key) {
                *dst = value::parse_int(v);
            }
        };
        set_int(&mut self.output_rate, "OutputRate");
        set_int(&mut self.output_bits, "OutputBits");
        set_int(&mut self.output_channels, "OutputChannels");
        set_int(&mut self.sample_count_2d, "SampleCount2D");
        set_int(&mut self.sample_count_3d, "SampleCount3D");
        set_int(&mut self.stream_count, "StreamCount");
        self.preferred_3d_providers.clear();
        for key in ["Preferred3DHW1", "Preferred3DHW2", "Preferred3DHW3", "Preferred3DHW4", "Preferred3DSW"] {
            if let Some(v) = b.value(key) {
                self.preferred_3d_providers.push(value::parse_string(v));
            }
        }
        set_str(&mut self.default_2d_speaker_type, "Default2DSpeakerType");
        set_str(&mut self.default_3d_speaker_type, "Default3DSpeakerType");
        let set_pct = |dst: &mut f32, key: &str| {
            if let Some(v) = b.value(key) {
                *dst = value::parse_percent(v);
            }
        };
        set_pct(&mut self.min_volume, "MinSampleVolume");
        if let Some(v) = b.value("Use3DSoundRangeVolumeFade") {
            self.use_3d_sound_range_volume_fade = value::parse_bool(v);
        }
        let set_real = |dst: &mut f32, key: &str| {
            if let Some(v) = b.value(key) {
                *dst = value::parse_real(v);
            }
        };
        set_real(&mut self.sound_range_volume_fade_exponent, "3DSoundRangeVolumeFadeExponent");
        set_int(&mut self.global_min_range, "GlobalMinRange");
        set_int(&mut self.global_max_range, "GlobalMaxRange");
        if let Some(v) = b.value("TimeBetweenDrawableSounds") {
            self.drawable_ambient_frames = value::parse_duration_frames(v);
        }
        if let Some(v) = b.value("TimeToFadeAudio") {
            self.fade_audio_frames = value::parse_duration_frames(v);
        }
        if let Some(v) = b.value("AudioFootprintInBytes") {
            self.max_cache_size = value::parse_int(v).max(0) as u32;
        }
        set_pct(&mut self.relative_2d_volume, "Relative2DVolume");
        set_pct(&mut self.default_sound_volume, "DefaultSoundVolume");
        set_pct(&mut self.default_3d_sound_volume, "Default3DSoundVolume");
        set_pct(&mut self.default_speech_volume, "DefaultSpeechVolume");
        set_pct(&mut self.default_music_volume, "DefaultMusicVolume");
        set_pct(&mut self.default_money_transaction_volume, "DefaultMoneyTransactionVolume");
        set_real(&mut self.microphone_desired_height_above_terrain, "MicrophoneDesiredHeightAboveTerrain");
        set_pct(&mut self.microphone_max_percentage_between_ground_and_camera, "MicrophoneMaxPercentageBetweenGroundAndCamera");
        set_real(&mut self.zoom_min_distance, "ZoomMinDistance");
        set_real(&mut self.zoom_max_distance, "ZoomMaxDistance");
        set_pct(&mut self.zoom_sound_volume_percentage_amount, "ZoomSoundVolumePercentageAmount");
    }

    /// Folder for a [`AudioType`] (`generateFilenamePrefix`).
    pub fn folder(&self, kind: AudioType) -> &str {
        match kind {
            AudioType::Music => &self.music_folder,
            AudioType::Streaming => &self.streaming_folder,
            AudioType::SoundEffect => &self.sounds_folder,
        }
    }

    /// `generateFilenameExtension`: sound effects get `.SoundsExtension`, music and
    /// streaming carry their extension in the `Filename` field.
    pub fn extension(&self, kind: AudioType) -> String {
        match kind {
            AudioType::Music | AudioType::Streaming => String::new(),
            AudioType::SoundEffect => format!(".{}", self.sounds_extension),
        }
    }

    /// `generateFilenamePrefix(type, localized)`: `AudioRoot\Folder\` with the
    /// language inserted for the localized variant.
    pub fn prefix(&self, kind: AudioType, localized: bool, language: &str) -> String {
        let folder = self.folder(kind);
        if localized {
            format!("{}\\{}\\{}\\", self.audio_root, folder, language)
        } else {
            format!("{}\\{}\\", self.audio_root, folder)
        }
    }

    /// Full non-localized path of `filename` (which includes any extension).
    pub fn base_path(&self, kind: AudioType, filename: &str) -> String {
        format!("{}{}", self.prefix(kind, false, ""), filename)
    }

    /// Full localized path of `filename` (`adjustForLocalization` candidate).
    pub fn localized_path(&self, kind: AudioType, filename: &str, language: &str) -> String {
        format!("{}{}", self.prefix(kind, true, language), filename)
    }
}
