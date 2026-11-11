//! The audio event database: every `AudioEvent` / `MusicTrack` / `DialogEvent`
//! after engine seeding, plus `AudioSettings` and `MiscAudio`.
//!
//! Lookup mirrors `AudioManager::newAudioEventInfo` / `findAudioEventInfo`
//! (`Core/GameEngine/Source/Common/Audio/GameAudio.cpp`): one namespace keyed by
//! name. `AudioEvent` blocks seed from `DefaultSoundEffect`, `MusicTrack` from
//! `DefaultMusicTrack`, `DialogEvent` from `DefaultDialog`
//! (`INIAudioEventInfo.cpp`).

use std::collections::BTreeMap;

use zr_ini::IniDb;

use crate::{AudioEventInfo, AudioSettings, AudioType};

/// One sound file a definition refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioReference {
    /// `"Sounds"`, `"SoundsNight"`, `"Attack"`, `"Filename"`, ...
    pub source: &'static str,
    /// The raw INI name (sound effects have no extension; music/dialog include it).
    pub raw: String,
}

/// A reference that exists in the VFS, with the path the engine would load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRef {
    /// File name including any extension (what `adjustForLocalization` sees).
    pub filename: String,
    /// Full VFS path (localized first, else the base folder).
    pub path: String,
    pub localized: bool,
}

/// Parsed audio tables.
#[derive(Debug, Clone, Default)]
pub struct AudioDatabase {
    pub settings: AudioSettings,
    pub language: String,
    events: Vec<AudioEventInfo>,
    index: BTreeMap<String, usize>,
    /// `MiscAudio` key -> audio event name, in file order.
    pub misc_audio: Vec<(String, String)>,
}

impl AudioDatabase {
    /// Builds the database from a merged [`IniDb`], as `AudioManager::init` does.
    /// `language` is the `Data\<language>` directory used by `adjustForLocalization`.
    pub fn from_ini(db: &IniDb, language: &str) -> AudioDatabase {
        let mut out = AudioDatabase {
            settings: AudioSettings::from_ini(db),
            language: language.to_owned(),
            ..Default::default()
        };
        for block in db.iter() {
            let Some(kind) = kind_of(&block.kind) else { continue };
            let name = block.name().unwrap_or("").to_owned();
            let i = match out.index.get(&name) {
                Some(&i) => i,
                None => {
                    // Seed from the kind's default (unless this *is* the default).
                    let mut info = if name == kind.default_name() {
                        AudioEventInfo::new(&name, kind)
                    } else {
                        out.index
                            .get(kind.default_name())
                            .map(|&d| out.events[d].clone())
                            .map(|mut e| {
                                e.name = name.clone();
                                e.sound_type = kind;
                                e
                            })
                            .unwrap_or_else(|| AudioEventInfo::new(&name, kind))
                    };
                    info.apply(block);
                    out.events.push(info);
                    let i = out.events.len() - 1;
                    out.index.insert(name, i);
                    continue;
                }
            };
            out.events[i].apply(block);
        }
        if let Some(b) = db.resolved("MiscAudio", "") {
            out.misc_audio =
                b.fields().map(|f| (f.key.clone(), crate::value::parse_string(&f.value))).collect();
        }
        out
    }

    /// Latest definition of an event by name (one namespace for all kinds).
    pub fn get(&self, name: &str) -> Option<&AudioEventInfo> {
        self.index.get(name).map(|&i| &self.events[i])
    }

    /// Every event in first-definition order (defaults included).
    pub fn events(&self) -> &[AudioEventInfo] {
        &self.events
    }

    /// Events of one kind, in definition order.
    pub fn by_kind(&self, kind: AudioType) -> impl Iterator<Item = &AudioEventInfo> {
        self.events.iter().filter(move |e| e.sound_type == kind)
    }

    /// `(audio_events, music_tracks, dialog_events)` including the `Default*` blocks.
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut n = (0, 0, 0);
        for e in &self.events {
            match e.sound_type {
                AudioType::SoundEffect => n.0 += 1,
                AudioType::Music => n.1 += 1,
                AudioType::Streaming => n.2 += 1,
            }
        }
        n
    }

    /// Sound file name a raw reference maps to: sound effects append
    /// `.SoundsExtension` (`generateFilenameExtension`), music/dialog carry it.
    pub fn reference_filename(&self, kind: AudioType, raw: &str) -> String {
        format!("{raw}{}", self.settings.extension(kind))
    }

    /// Every sound file this event references, in engine order.
    pub fn references(&self, event: &AudioEventInfo) -> Vec<AudioReference> {
        let mut out = Vec::new();
        match event.sound_type {
            AudioType::Music | AudioType::Streaming => {
                if let Some(f) = &event.filename {
                    if !f.is_empty() {
                        out.push(AudioReference { source: "Filename", raw: f.clone() });
                    }
                }
            }
            AudioType::SoundEffect => {
                for (source, list) in [
                    ("Sounds", &event.sounds),
                    ("SoundsNight", &event.sounds_night),
                    ("SoundsEvening", &event.sounds_evening),
                    ("SoundsMorning", &event.sounds_morning),
                    ("Attack", &event.attack),
                    ("Decay", &event.decay),
                ] {
                    for raw in list {
                        out.push(AudioReference { source, raw: raw.clone() });
                    }
                }
            }
        }
        out
    }

    /// `AudioEventRTS::generateFilename` + `adjustForLocalization`: returns the
    /// localized file when it exists, else the base folder file, else `None`.
    /// `exists` is the client's VFS probe (e.g. `vfs.read_archived(p).is_some()`).
    pub fn resolve_reference(
        &self,
        kind: AudioType,
        raw: &str,
        exists: &dyn Fn(&str) -> bool,
    ) -> Option<ResolvedRef> {
        let filename = self.reference_filename(kind, raw);
        let localized = self.settings.localized_path(kind, &filename, &self.language);
        if exists(&localized) {
            return Some(ResolvedRef { filename, path: localized, localized: true });
        }
        let base = self.settings.base_path(kind, &filename);
        if exists(&base) {
            return Some(ResolvedRef { filename, path: base, localized: false });
        }
        None
    }
}

/// Block kind -> audio type (`parseAudioEventDefinition` etc.).
pub fn kind_of(block_kind: &str) -> Option<AudioType> {
    match block_kind {
        "AudioEvent" => Some(AudioType::SoundEffect),
        "MusicTrack" => Some(AudioType::Music),
        "DialogEvent" => Some(AudioType::Streaming),
        _ => None,
    }
}
