//! `AudioEventInfo` (`Core/GameEngine/Include/Common/AudioEventInfo.h`) and its
//! INI field table, ported from
//! `Core/GameEngine/Source/Common/INI/INIAudioEventInfo.cpp`.
//!
//! `AudioEvent`, `MusicTrack` and `DialogEvent` all produce one `AudioEventInfo`,
//! seeded from `DefaultSoundEffect` / `DefaultMusicTrack` / `DefaultDialog`
//! respectively before their own fields are applied.

use zr_ini::Block;

use crate::value;

/// `AudioType` (`AudioEventInfo.h`): chosen by the INI block kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioType {
    Music,
    Streaming,
    SoundEffect,
}

impl AudioType {
    /// The default block each kind is seeded from (`parseAudioEventDefinition` etc.).
    pub fn default_name(self) -> &'static str {
        match self {
            AudioType::Music => "DefaultMusicTrack",
            AudioType::Streaming => "DefaultDialog",
            AudioType::SoundEffect => "DefaultSoundEffect",
        }
    }
}

/// `AudioPriority` (`AudioEventInfo.h`), `theAudioPriorityNames` order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AudioPriority {
    Lowest,
    Low,
    Normal,
    High,
    Critical,
}

/// `theSoundTypeNames` bit positions (`parseBitString32`).
pub mod sound_type {
    pub const UI: u32 = 0x0001;
    pub const WORLD: u32 = 0x0002;
    pub const SHROUDED: u32 = 0x0004;
    pub const GLOBAL: u32 = 0x0008;
    pub const VOICE: u32 = 0x0010;
    pub const PLAYER: u32 = 0x0020;
    pub const ALLIES: u32 = 0x0040;
    pub const ENEMIES: u32 = 0x0080;
    pub const EVERYONE: u32 = 0x0100;

    /// Names in `theSoundTypeNames` order.
    pub const NAMES: &[&str] =
        &["UI", "WORLD", "SHROUDED", "GLOBAL", "VOICE", "PLAYER", "ALLIES", "ENEMIES", "EVERYONE"];
}

/// `theAudioControlNames` bit positions.
pub mod control {
    pub const LOOP: u32 = 0x0001;
    pub const RANDOM: u32 = 0x0002;
    pub const ALL: u32 = 0x0004;
    pub const POSTDELAY: u32 = 0x0008;
    pub const INTERRUPT: u32 = 0x0010;

    pub const NAMES: &[&str] = &["LOOP", "RANDOM", "ALL", "POSTDELAY", "INTERRUPT"];
}

/// `AudioEventInfo`: one `AudioEvent`, `MusicTrack` or `DialogEvent`.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioEventInfo {
    pub name: String,
    pub sound_type: AudioType,
    /// `Filename` (music tracks / dialog events); sound effects use `sounds`.
    pub filename: Option<String>,

    pub volume: f32,
    pub volume_shift: f32,
    pub min_volume: f32,
    /// `parsePitchShift`: stored as multipliers, `1 + percent/100`.
    pub pitch_shift_min: f32,
    pub pitch_shift_max: f32,
    pub delay_min: i32,
    pub delay_max: i32,
    pub limit: i32,
    pub loop_count: i32,
    pub priority: AudioPriority,
    pub type_bits: u32,
    pub control_bits: u32,

    pub sounds: Vec<String>,
    pub sounds_night: Vec<String>,
    pub sounds_evening: Vec<String>,
    pub sounds_morning: Vec<String>,
    pub attack: Vec<String>,
    pub decay: Vec<String>,

    pub low_pass_cutoff: f32,
    pub min_distance: f32,
    pub max_distance: f32,
}

impl AudioEventInfo {
    /// A fresh instance with the C++ zero/constructor defaults for its kind.
    pub fn new(name: &str, sound_type: AudioType) -> AudioEventInfo {
        AudioEventInfo {
            name: name.to_owned(),
            sound_type,
            filename: None,
            volume: 1.0,
            volume_shift: 0.0,
            min_volume: 0.0,
            pitch_shift_min: 1.0,
            pitch_shift_max: 1.0,
            delay_min: 0,
            delay_max: 0,
            limit: 0,
            loop_count: 0,
            priority: AudioPriority::Lowest,
            type_bits: 0,
            control_bits: 0,
            sounds: Vec::new(),
            sounds_night: Vec::new(),
            sounds_evening: Vec::new(),
            sounds_morning: Vec::new(),
            attack: Vec::new(),
            decay: Vec::new(),
            low_pass_cutoff: 1.0,
            min_distance: 0.0,
            max_distance: 0.0,
        }
    }

    /// Applies the fields present in `b` on top of the current values, mirroring
    /// `INI::initFromINI(.., m_audioEventInfo)`. List fields are cleared first
    /// (`parseSoundsList` clears the vector).
    pub fn apply(&mut self, b: &Block) {
        if let Some(v) = b.value("Filename") {
            self.filename = Some(value::parse_string(v));
        }
        let pct = |dst: &mut f32, key: &str| {
            if let Some(v) = b.value(key) {
                *dst = value::parse_percent(v);
            }
        };
        pct(&mut self.volume, "Volume");
        pct(&mut self.volume_shift, "VolumeShift");
        pct(&mut self.min_volume, "MinVolume");
        if let Some(v) = b.value("PitchShift") {
            let (min, max) = value::parse_pitch_shift(v);
            self.pitch_shift_min = min;
            self.pitch_shift_max = max;
        }
        if let Some(v) = b.value("Delay") {
            let (min, max) = value::parse_delay(v);
            self.delay_min = min;
            self.delay_max = max;
        }
        if let Some(v) = b.value("Limit") {
            self.limit = value::parse_int(v);
        }
        if let Some(v) = b.value("LoopCount") {
            self.loop_count = value::parse_int(v);
        }
        if let Some(v) = b.value("Priority") {
            self.priority = value::parse_priority(v);
        }
        if let Some(v) = b.value("Type") {
            self.type_bits = value::parse_bits(v, sound_type::NAMES);
        }
        if let Some(v) = b.value("Control") {
            self.control_bits = value::parse_bits(v, control::NAMES);
        }
        let list = |dst: &mut Vec<String>, key: &str| {
            if let Some(v) = b.value(key) {
                *dst = value::parse_sounds_list(v);
            }
        };
        list(&mut self.sounds, "Sounds");
        list(&mut self.sounds_night, "SoundsNight");
        list(&mut self.sounds_evening, "SoundsEvening");
        list(&mut self.sounds_morning, "SoundsMorning");
        list(&mut self.attack, "Attack");
        list(&mut self.decay, "Decay");
        pct(&mut self.low_pass_cutoff, "LowPassCutoff");
        if let Some(v) = b.value("MinRange") {
            self.min_distance = value::parse_real(v);
        }
        if let Some(v) = b.value("MaxRange") {
            self.max_distance = value::parse_real(v);
        }
    }

    /// Every sound-effect file base name this event references (all four
    /// time-of-day lists plus attack and decay). Music / streaming events
    /// reference only [`AudioEventInfo::filename`].
    pub fn sound_names(&self) -> impl Iterator<Item = &str> {
        self.sounds
            .iter()
            .chain(&self.sounds_night)
            .chain(&self.sounds_evening)
            .chain(&self.sounds_morning)
            .chain(&self.attack)
            .chain(&self.decay)
            .map(String::as_str)
    }

    /// `BitIsSet(control, AC_RANDOM)`.
    pub fn is_random(&self) -> bool {
        self.control_bits & control::RANDOM != 0
    }

    /// `BitIsSet(control, AC_LOOP)`.
    pub fn is_looping(&self) -> bool {
        self.control_bits & control::LOOP != 0
    }

    /// Deterministic variant choice, ported from `AudioEventRTS::generateFilename`:
    /// `AC_RANDOM` uses the supplied client RNG (never the sim RNG), else the
    /// playing index advances sequentially. `playing_index` is the per-event
    /// runtime state (`m_playingAudioIndex`, starts at `-1`).
    pub fn choose_index(&self, playing_index: &mut i32, rng: &mut impl FnMut(u32, u32) -> u32) -> Option<usize> {
        let n = self.sounds.len();
        if n == 0 {
            *playing_index = -1;
            return None;
        }
        let n_i = n as i32;
        if self.is_random() {
            let mut which = rng(0, n as u32 - 1) as i32;
            // Avoid repeating the previous pick when there are more than two choices.
            if which == *playing_index && n > 2 {
                which = (which + 1) % n_i;
            }
            *playing_index = which;
            Some(which as usize)
        } else {
            *playing_index = (*playing_index + 1).rem_euclid(n_i);
            Some(*playing_index as usize)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(sounds: &[&str], control_bits: u32) -> AudioEventInfo {
        let mut e = AudioEventInfo::new("E", AudioType::SoundEffect);
        e.sounds = sounds.iter().map(|s| (*s).to_owned()).collect();
        e.control_bits = control_bits;
        e
    }

    #[test]
    fn sequential_advances_then_wraps() {
        let e = event(&["a", "b", "c"], 0);
        let mut idx = -1;
        let mut rng = |_, _| 0;
        let picks: Vec<_> = (0..4).map(|_| e.choose_index(&mut idx, &mut rng).unwrap()).collect();
        assert_eq!(picks, [0, 1, 2, 0]);
    }

    #[test]
    fn random_uses_supplied_rng_and_avoids_repeat() {
        let e = event(&["a", "b", "c"], control::RANDOM);
        let mut idx = -1;
        // A fixed RNG of 0 would otherwise repeat; >2 choices bumps it.
        let mut rng = |_, _| 0;
        assert_eq!(e.choose_index(&mut idx, &mut rng), Some(0));
        assert_eq!(e.choose_index(&mut idx, &mut rng), Some(1));
        assert_eq!(e.choose_index(&mut idx, &mut rng), Some(0));
    }

    #[test]
    fn empty_list_has_no_choice() {
        let e = event(&[], 0);
        let mut idx = 0;
        assert_eq!(e.choose_index(&mut idx, &mut |_, _| 0), None);
        assert_eq!(idx, -1);
    }
}
