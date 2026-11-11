//! Client-side audio: resolve `AudioEvent` names to WAVs in the VFS, decode IMA ADPCM, and play
//! them through Bevy audio.
//!
//! Ported from `Core/GameEngine/Source/Common/Audio/AudioEventRTS.cpp` (variant/pitch/volume
//! shift), `GameAudio.cpp` (`addAudioEvent` culling) and
//! `Core/GameEngineDevice/Source/MilesAudioDevice/MilesAudioManager.cpp` (`getEffectiveVolume`
//! range fade, `doesViolateLimit`, `isObjectPlayingVoice`, `isPlayingLowerPriority`).
//!
//! The client only reads sim state; audio is presentation, so nothing here feeds the sim. The
//! variant choice and pitch/volume shift use a client RNG (`GameAudioRandomValue`), never the
//! sim RNG.

use std::collections::{BTreeMap, HashMap, HashSet};

use bevy::audio::{AudioPlayer, AudioSource, PlaybackSettings, Volume};
use bevy::prelude::*;
use zr_assets::Vfs;
use zr_audio::wav::{decode_ima_adpcm, parse_wav, to_pcm_wav, WavFormat};
use zr_audio::{control, sound_type, AudioDatabase, AudioEventInfo, AudioPriority, AudioType};
use zr_ini::IniDb;
use zr_sim::ObjectId;

use crate::sim_view::SimRes;

/// One request to play an `AudioEvent`/`MusicTrack` by name.
#[derive(Clone, Debug)]
pub struct AudioRequest {
    pub event: String,
    /// `ObjectID` (`AudioEventRTS::setObjectID`) for voice/limit tracking.
    pub object: Option<ObjectId>,
    /// World position (Generals space) for positional audio; `None` is 2-D.
    pub pos: Option<[f32; 3]>,
}

/// Client systems push requests here; [`audio_play`] drains them once per frame.
#[derive(Resource, Default)]
pub struct AudioQueue(pub Vec<AudioRequest>);

impl AudioQueue {
    pub fn push(&mut self, event: impl Into<String>, object: Option<ObjectId>, pos: Option<[f32; 3]>) {
        self.0.push(AudioRequest { event: event.into(), object, pos });
    }
}

/// A currently-playing sound, for limit / priority / voice tracking.
struct Active {
    event: String,
    object: Option<ObjectId>,
    priority: AudioPriority,
    voice: bool,
    entity: Entity,
    start: u64,
}

/// The parsed audio database plus the runtime state the engine keeps in `AudioManager`.
#[derive(Resource)]
pub struct AudioBank {
    db: AudioDatabase,
    muted: bool,
    /// Master volumes (`m_soundVolume` / `m_sound3DVolume` / `m_speechVolume` / `m_musicVolume`),
    /// seeded from `AudioSettings` defaults.
    sound_volume: f32,
    sound3d_volume: f32,
    speech_volume: f32,
    music_volume: f32,
    min_volume: f32,
    fade_exponent: f32,
    use_range_fade: bool,
    global_min_range: f32,
    global_max_range: f32,
    /// Per-event sequential variant index (`m_playingAudioIndex`, starts at `-1`).
    playing_index: BTreeMap<String, i32>,
    /// Client RNG seed (`GameAudioRandomValue`).
    rng: u32,
    /// Resolved path -> decoded asset.
    cache: HashMap<String, Handle<AudioSource>>,
    active: Vec<Active>,
    frame: u64,
    /// Previous frame's objects (`id -> (template, pos)`) for death detection.
    known: BTreeMap<ObjectId, (String, [f32; 3])>,
    /// Previous frame's live projectile ids for `FireSound` detection.
    projectiles: HashSet<u32>,
    /// Previous local selection for `VoiceSelect`.
    selected: Vec<ObjectId>,
    /// Music playlist (event names) and current index.
    music: Vec<String>,
    music_index: usize,
    music_entity: Option<Entity>,
}

impl AudioBank {
    pub fn new(ini: &IniDb, language: &str, muted: bool) -> AudioBank {
        let db = AudioDatabase::from_ini(ini, language);
        let (sound_volume, sound3d_volume, speech_volume, music_volume, min_volume, fade_exponent, use_range_fade, global_min_range, global_max_range) = {
            let s = &db.settings;
            (
                s.default_sound_volume,
                s.default_3d_sound_volume,
                s.default_speech_volume,
                s.default_music_volume,
                s.min_volume,
                s.sound_range_volume_fade_exponent,
                s.use_3d_sound_range_volume_fade,
                s.global_min_range as f32,
                s.global_max_range as f32,
            )
        };
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x1234_5678, |d| d.as_nanos() as u32);
        AudioBank {
            db,
            muted,
            sound_volume,
            sound3d_volume,
            speech_volume,
            music_volume,
            min_volume,
            fade_exponent,
            use_range_fade,
            global_min_range,
            global_max_range,
            playing_index: BTreeMap::new(),
            rng: seed | 1,
            cache: HashMap::new(),
            active: Vec::new(),
            frame: 0,
            known: BTreeMap::new(),
            projectiles: HashSet::new(),
            selected: Vec::new(),
            music: Vec::new(),
            music_index: 0,
            music_entity: None,
        }
    }

    /// Applies the options menu's SFX / music / voice volumes (`saveOptions`): the 2D sound is the
    /// slider scaled by a negative `Relative2DVolume`, the 3D sound by a positive one.
    pub fn set_volumes(&mut self, sfx: f32, music: f32, voice: f32) {
        let rel = self.db.settings.relative_2d_volume.clamp(-1.0, 1.0);
        let sfx = sfx.clamp(0.0, 1.0);
        self.sound_volume = if rel < 0.0 { sfx * (1.0 + rel) } else { sfx };
        self.sound3d_volume = if rel > 0.0 { sfx * (1.0 - rel) } else { sfx };
        self.music_volume = music.clamp(0.0, 1.0);
        self.speech_volume = voice.clamp(0.0, 1.0);
    }

    /// The AudioSettings defaults for the music, SFX (the larger of 2D and 3D, which is what the
    /// options slider shows) and voice volumes, as fractions.
    pub fn volume_defaults(&self) -> (f32, f32, f32) {
        let s = &self.db.settings;
        (s.default_music_volume, s.default_sound_volume.max(s.default_3d_sound_volume), s.default_speech_volume)
    }

    /// AudioSettings `Relative2DVolume` (`-0.1` in the shipped data).
    pub fn relative_2d_volume(&self) -> f32 {
        self.db.settings.relative_2d_volume
    }

    /// `AudioEventRTS::isCurrentlyPlaying`: whether an event of this name is still active (the
    /// EVA speech gate). Retained entries are pruned each frame against the live `AudioPlayer`s.
    pub fn is_playing(&self, event: &str) -> bool {
        self.active.iter().any(|a| a.event == event)
    }

    fn rand(&mut self, lo: u32, hi: u32) -> u32 {
        // `GameAudioRandomValue` (inclusive); a plain LCG is enough for presentation.
        self.rng = self.rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        if hi <= lo {
            return lo;
        }
        lo + (self.rng >> 8) % (hi - lo + 1)
    }

    fn rand_real(&mut self, lo: f32, hi: f32) -> f32 {
        if hi <= lo {
            return lo;
        }
        let t = self.rand(0, 1000) as f32 / 1000.0;
        lo + t * (hi - lo)
    }

    /// `AudioManager::getEffectiveVolume` (`MilesAudioManager.cpp`).
    fn effective_volume(&self, info: &AudioEventInfo, positional: bool, dist: f32) -> f32 {
        let mut volume = info.volume;
        match info.sound_type {
            AudioType::Music => volume *= self.music_volume,
            AudioType::Streaming => volume *= self.speech_volume,
            AudioType::SoundEffect => {
                if positional {
                    volume *= self.sound3d_volume;
                    let (min, max) = if info.type_bits & sound_type::GLOBAL != 0 {
                        (self.global_min_range, self.global_max_range)
                    } else {
                        (info.min_distance, info.max_distance)
                    };
                    // Retail events set `MaxRange`; treat an unset range as unlimited so the
                    // viewer is not silent (the engine would cull at distance >= max).
                    if max > min {
                        if dist >= max {
                            return 0.0;
                        }
                        if self.use_range_fade && dist > min {
                            let a = ((dist - min) / (max - min)).powf(self.fade_exponent);
                            volume *= 1.0 - a;
                        }
                    }
                } else {
                    volume *= self.sound_volume;
                }
            }
        }
        volume
    }

    /// Resolves and caches the asset for one file, decoding IMA ADPCM to PCM.
    fn load(&mut self, path: &str, vfs: &Vfs, assets: &mut Assets<AudioSource>) -> Option<Handle<AudioSource>> {
        if let Some(h) = self.cache.get(path) {
            return Some(h.clone());
        }
        let raw = vfs.read(path)?;
        let bytes = match parse_wav(&raw) {
            Ok(info) if info.format == WavFormat::ImaAdpcm => {
                let pcm = decode_ima_adpcm(&info, &raw).ok()?;
                to_pcm_wav(&info, &pcm)
            }
            Ok(info) if info.format == WavFormat::Pcm => raw.into_owned(),
            // MS ADPCM / other tags: rodio cannot decode them; skip rather than panic.
            _ => return None,
        };
        let handle = assets.add(AudioSource { bytes: bytes.into() });
        self.cache.insert(path.to_owned(), handle.clone());
        Some(handle)
    }

    /// The file for one play of `info`: variant choice then localization (`AudioEventRTS`).
    fn choose_path(&mut self, info: &AudioEventInfo, vfs: &Vfs) -> Option<String> {
        let exists = |p: &str| vfs.read(p).is_some();
        match info.sound_type {
            AudioType::SoundEffect => {
                let mut idx = self.playing_index.get(&info.name).copied().unwrap_or(-1);
                let mut seed = self.rng;
                let picked = info.choose_index(&mut idx, &mut |lo, hi| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    if hi <= lo {
                        lo
                    } else {
                        lo + (seed >> 8) % (hi - lo + 1)
                    }
                })?;
                self.rng = seed;
                self.playing_index.insert(info.name.clone(), idx);
                let raw = info.sounds.get(picked)?;
                self.db.resolve_reference(AudioType::SoundEffect, raw, &exists).map(|r| r.path)
            }
            AudioType::Music | AudioType::Streaming => {
                let raw = info.filename.as_deref()?;
                self.db.resolve_reference(info.sound_type, raw, &exists).map(|r| r.path)
            }
        }
    }

    /// `SoundManager::canPlayNow` (limit / voice / interrupt) plus priority.
    fn allowed(&mut self, event: &str, object: Option<ObjectId>, info: &AudioEventInfo) -> bool {
        let interrupt = info.control_bits & control::INTERRUPT != 0;
        // Voice: one voice per object at a time (`violatesVoice`).
        if info.type_bits & sound_type::VOICE != 0 {
            if let Some(id) = object {
                if self.active.iter().any(|a| a.voice && a.object == Some(id)) && !interrupt {
                    return false;
                }
            }
        }
        // Limit: at most `Limit` of this event name (`doesViolateLimit`).
        if info.limit > 0 {
            let n = self.active.iter().filter(|a| a.event == event).count() as i32;
            if n >= info.limit {
                if interrupt {
                    return true;
                }
                // Kill the oldest lower-priority instance of the same event if possible.
                if let Some(pos) = self
                    .active
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| a.event == event && a.priority < info.priority)
                    .min_by_key(|(_, a)| a.start)
                    .map(|(i, _)| i)
                {
                    self.active.remove(pos);
                    return true;
                }
                return false;
            }
        }
        true
    }

    /// Spawns the `AudioPlayer` and records it for limit/priority tracking.
    #[allow(clippy::too_many_arguments)]
    fn play(
        &mut self,
        req: &AudioRequest,
        vfs: &Vfs,
        assets: &mut Assets<AudioSource>,
        commands: &mut Commands,
        listener: Vec3,
    ) -> Option<Entity> {
        let info = self.db.get(&req.event)?.clone();
        // `addAudioEvent`: cull muted audio.
        if info.volume < self.min_volume {
            return None;
        }
        let positional = req.pos.is_some() && info.type_bits & sound_type::WORLD != 0;
        let dist = req.pos.map_or(0.0, |p| (crate::convert::to_bevy(p) - listener).length());
        if !self.allowed(&req.event, req.object, &info) {
            return None;
        }
        let volume = self.effective_volume(&info, positional, dist);
        if volume <= 0.0 {
            return None;
        }
        let path = self.choose_path(&info, vfs)?;
        let handle = self.load(&path, vfs, assets)?;
        let shift = self.rand_real(1.0 + info.volume_shift, 1.0);
        let pitch = self.rand_real(info.pitch_shift_min, info.pitch_shift_max);
        let settings = PlaybackSettings::DESPAWN
            .with_volume(Volume::Linear(volume * shift))
            .with_speed(pitch)
            .with_spatial(positional);
        let entity = commands.spawn((AudioPlayer::new(handle), settings)).id();
        self.active.push(Active {
            event: req.event.clone(),
            object: req.object,
            priority: info.priority,
            voice: info.type_bits & sound_type::VOICE != 0,
            entity,
            start: self.frame,
        });
        Some(entity)
    }

    /// Music playlist: advances when the current track finishes (`PlaybackMode::Despawn`).
    fn update_music(
        &mut self,
        alive: &HashSet<Entity>,
        vfs: &Vfs,
        assets: &mut Assets<AudioSource>,
        commands: &mut Commands,
    ) {
        if self.music.is_empty() {
            self.music = self
                .db
                .by_kind(AudioType::Music)
                .filter(|e| e.filename.as_deref().is_some_and(|f| !f.is_empty()))
                .map(|e| e.name.clone())
                .collect();
        }
        if self.music.is_empty() || self.music_entity.is_some_and(|e| alive.contains(&e)) {
            return;
        }
        let n = self.music.len();
        let name = self.music[self.music_index % n].clone();
        self.music_index = self.music_index.wrapping_add(1);
        let req = AudioRequest { event: name, object: None, pos: None };
        if let Some(e) = self.play(&req, vfs, assets, commands, Vec3::ZERO) {
            self.music_entity = Some(e);
        } else if self.music_index >= n * 2 {
            // Nothing in the playlist resolves (no files); stop retrying.
            self.music.clear();
        }
    }
}

pub fn add_systems(app: &mut App) {
    app.init_resource::<AudioQueue>();
    app.add_systems(Update, audio_play.after(crate::sim_view::sim_step).run_if(in_state(crate::shell::ShellState::InGame)));
}

/// Drains [`AudioQueue`], detects selection/death/fire events, and plays everything.
#[allow(clippy::too_many_arguments)]
fn audio_play(
    mut bank: ResMut<AudioBank>,
    mut queue: ResMut<AudioQueue>,
    mut assets: ResMut<Assets<AudioSource>>,
    mut commands: Commands,
    cache: Option<Res<crate::sim_view::ModelCache>>,
    sim: Option<NonSend<SimRes>>,
    cam: Query<&GlobalTransform, With<Camera3d>>,
    live: Query<Entity, With<AudioPlayer<AudioSource>>>,
) {
    let Some(cache) = cache else { return };
    bank.frame += 1;
    let listener = cam.iter().next().map_or(Vec3::ZERO, |t| t.translation());
    let alive: HashSet<Entity> = live.iter().collect();
    bank.active.retain(|a| alive.contains(&a.entity));

    if bank.muted {
        queue.0.clear();
        return;
    }

    let mut requests = std::mem::take(&mut queue.0);

    if let Some(sim) = &sim {
        collect_sim_events(&mut bank, sim, &mut requests);
    }

    let vfs = &cache.vfs;
    for req in &requests {
        bank.play(req, vfs, &mut assets, &mut commands, listener);
    }
    bank.update_music(&alive, vfs, &mut assets, &mut commands);
}

/// Selection (`VoiceSelect`), death (`SoundDie`) and firing (`FireSound`) detection.
fn collect_sim_events(bank: &mut AudioBank, sim: &SimRes, out: &mut Vec<AudioRequest>) {
    let templates = sim.game.data().templates();

    // Selection: newly selected local objects say their `VoiceSelect`.
    for &id in &sim.selected {
        if bank.selected.contains(&id) {
            continue;
        }
        if let Some(o) = sim.game.object(id) {
            if let Some(ev) = templates.thing(o.template).and_then(|t| t.sounds.get("VoiceSelect")) {
                out.push(AudioRequest { event: ev.clone(), object: Some(id), pos: Some([o.position.x, o.position.y, o.position.z]) });
            }
        }
    }
    bank.selected = sim.selected.clone();

    // Deaths: objects present last frame and gone now say their `SoundDie`.
    let mut current: BTreeMap<ObjectId, (String, [f32; 3])> = BTreeMap::new();
    for o in sim.game.objects() {
        current.insert(o.id, (o.template.to_owned(), [o.position.x, o.position.y, o.position.z]));
    }
    for (id, (template, pos)) in &bank.known {
        if current.contains_key(id) {
            continue;
        }
        if let Some(ev) = templates.thing(template).and_then(|t| t.sounds.get("SoundDie")) {
            out.push(AudioRequest { event: ev.clone(), object: Some(*id), pos: Some(*pos) });
        }
    }
    // Births: a unit that just left a factory says its `VoiceCreate`
    // (`ProductionUpdate.cpp:806`; buildings are excluded because they are created under
    // construction). Skipped on the first frame, when every object is "new".
    if !bank.known.is_empty() {
        for (id, (template, pos)) in &current {
            if bank.known.contains_key(id) {
                continue;
            }
            let unit = sim
                .game
                .logic()
                .find_object(*id)
                .is_some_and(|o| !o.template.is_kind_of(zr_sim::template::kind_of::STRUCTURE) && !o.under_construction);
            if !unit || sim.game.object(*id).is_none_or(|o| o.owner != sim.local) {
                continue;
            }
            if let Some(ev) = templates.thing(template).and_then(|t| t.sounds.get("VoiceCreate")) {
                out.push(AudioRequest { event: ev.clone(), object: Some(*id), pos: Some(*pos) });
            }
        }
    }
    bank.known = current;

    // Firing: a projectile's creation frame plays the shooter's current weapon `FireSound`.
    let mut live_ids = HashSet::new();
    for p in sim.game.projectiles() {
        live_ids.insert(p.id);
        if bank.projectiles.contains(&p.id) {
            continue;
        }
        let Some(o) = sim.game.object(p.source) else { continue };
        let sound = sim.game.logic().find_object(p.source).and_then(|obj| {
            obj.combat
                .weapons
                .weapons
                .iter()
                .flatten()
                .find_map(|w| templates.weapon(&w.template.name).and_then(|wt| wt.fire_sound.clone()))
        });
        if let Some(ev) = sound {
            out.push(AudioRequest { event: ev, object: Some(p.source), pos: Some([o.position.x, o.position.y, o.position.z]) });
        }
    }
    bank.projectiles = live_ids;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bank() -> AudioBank {
        let ini = IniDb::new();
        let mut b = AudioBank::new(&ini, "English", false);
        b.sound_volume = 0.8;
        b.sound3d_volume = 1.0;
        b.music_volume = 0.5;
        b.speech_volume = 0.9;
        b.use_range_fade = true;
        b.fade_exponent = 4.0;
        b
    }

    fn effect() -> AudioEventInfo {
        let mut e = AudioEventInfo::new("E", AudioType::SoundEffect);
        e.volume = 1.0;
        e.min_distance = 100.0;
        e.max_distance = 200.0;
        e
    }

    #[test]
    fn effective_volume_2d_uses_sound_volume() {
        let b = bank();
        let mut e = effect();
        e.type_bits = 0;
        assert!((b.effective_volume(&e, false, 0.0) - 0.8).abs() < 1e-6);
    }

    #[test]
    fn effective_volume_3d_range_fade_matches_engine() {
        let b = bank();
        let mut e = effect();
        e.type_bits = sound_type::WORLD;
        // Inside MinRange: full 3-D volume.
        assert!((b.effective_volume(&e, true, 50.0) - 1.0).abs() < 1e-6);
        // At MaxRange: culled.
        assert_eq!(b.effective_volume(&e, true, 200.0), 0.0);
        // Half-way: attenuation = 0.5^4, so volume = 1 - 0.0625.
        let mid = b.effective_volume(&e, true, 150.0);
        assert!((mid - 0.9375).abs() < 1e-6, "got {mid}");
    }

    #[test]
    fn music_and_speech_scale_by_their_volumes() {
        let b = bank();
        let mut e = effect();
        e.type_bits = 0;
        e.sound_type = AudioType::Music;
        assert!((b.effective_volume(&e, false, 0.0) - 0.5).abs() < 1e-6);
        e.sound_type = AudioType::Streaming;
        assert!((b.effective_volume(&e, false, 0.0) - 0.9).abs() < 1e-6);
    }

    #[test]
    fn rand_stays_in_bounds() {
        let mut b = bank();
        for _ in 0..100 {
            let v = b.rand(3, 7);
            assert!((3..=7).contains(&v));
            let f = b.rand_real(1.5, 2.0);
            assert!((1.5..=2.0).contains(&f));
        }
    }
}
