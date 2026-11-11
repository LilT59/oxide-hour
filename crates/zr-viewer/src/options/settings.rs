//! The options screen's settings model, without any Bevy UI.
//!
//! * `options.ini` uses the C++ key names and value formats (`OptionPreferences::get*` for the
//!   reads, `OptionsMenu.cpp` `saveOptions` for the writes). The file is written sorted, one
//!   `Key = Value` per line, like `UserPreferences::write`.
//! * Only what this screen edits is typed. Every other key (IP addresses, the firewall port,
//!   `ArchiveReplays`, ...) stays in [`GameSettings::raw`] and is written back as it was read.
//! * The detail presets: `Low`, `Medium` and `High` come from the install's `StaticGameLOD`
//!   blocks (`GameLOD.cpp` `applyStaticLODLevel` sets the same fields), `VeryHigh` from the table
//!   hard-coded in `GameLODManager::GameLODManager` (`GameLOD.cpp:251-273`).
//! * [`Session`] is the ACCEPT / CANCEL model: the screen edits a draft, the applied settings
//!   change only on ACCEPT.

use std::collections::BTreeMap;

use bevy::prelude::Resource;
use zr_fx::ParticlePriority;
use zr_ini::{Block, IniDb};

use crate::convert::{FILTER_BILINEAR, FILTER_NONE};
use crate::userdata;

/// The window size when `Resolution` is absent (the viewer's own default, not a retail value).
pub const DEFAULT_RESOLUTION: (u32, u32) = (1600, 900);

/// Volumes used until the game reads the install's `AudioSettings` (`DefaultMusicVolume` 55%,
/// `DefaultSoundVolume` and `Default3DSoundVolume` 80%, `DefaultSpeechVolume` 70%). Percent.
const FALLBACK_MUSIC: i32 = 55;
const FALLBACK_SFX: i32 = 80;
const FALLBACK_VOICE: i32 = 70;

/// The detail combo (`StaticGameLODLevel`, names from `StaticGameLODNames`), in combo order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    Low,
    Medium,
    High,
    VeryHigh,
    Custom,
}

impl Detail {
    pub const ALL: [Detail; 5] = [Detail::Low, Detail::Medium, Detail::High, Detail::VeryHigh, Detail::Custom];

    /// The `options.ini` name (`StaticGameLODNames`).
    pub fn key(self) -> &'static str {
        match self {
            Detail::Low => "Low",
            Detail::Medium => "Medium",
            Detail::High => "High",
            Detail::VeryHigh => "VeryHigh",
            Detail::Custom => "Custom",
        }
    }

    /// Parses a `StaticGameLOD` value (`GameLODManager::getStaticGameLODIndex`, ignoring case).
    pub fn from_key(s: &str) -> Option<Detail> {
        Self::ALL.into_iter().find(|d| d.key().eq_ignore_ascii_case(s.trim()))
    }

    /// The combo index.
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|d| *d == self).unwrap_or(0)
    }

    /// The next entry in combo order (the combo is drawn as a cycle).
    pub fn next(self) -> Detail {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }
}

/// The advanced display values (`OptionsMenu.cpp` `saveOptions`, the `Custom` branch). The
/// viewer draws with the ones it has an effect for; the rest are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CustomLod {
    /// `TextureReduction`, 0..=2 (the LowRes slider shows `2 - reduction`).
    pub texture_reduction: i32,
    /// `UseShadowVolumes` (3D shadows; the viewer has no stencil volumes).
    pub shadow_volumes: bool,
    /// `UseShadowDecals` (2D shadows; `shadow.rs`).
    pub shadow_decals: bool,
    pub cloud_map: bool,
    pub light_map: bool,
    pub soft_water: bool,
    /// `ExtraAnimations` (yes = extra animations on, i.e. `!m_useDrawModuleLOD`).
    pub extra_animations: bool,
    /// `DynamicLOD`. Only the `Custom` detail stores its own value: the static presets always
    /// enable dynamic LOD (`StaticGameLODInfo::m_enableDynamicLOD`, `GameLOD.cpp:109`, `:272`).
    pub dynamic_lod: bool,
    pub heat_effects: bool,
    /// `BuildingOcclusion` (behind-building markers).
    pub building_occlusion: bool,
    /// `ShowTrees`.
    pub show_trees: bool,
    /// `MaxParticleCount`, at least 100 (`OptionPreferences::getParticleCap`).
    pub max_particles: i32,
}

impl Default for CustomLod {
    /// The VeryHigh values (`GameLOD.cpp:251-273`). VeryHigh does not set the behind-building,
    /// dynamic-LOD or trees globals, so those default on here, which is a viewer choice.
    fn default() -> Self {
        CustomLod {
            texture_reduction: 0,
            shadow_volumes: true,
            shadow_decals: true,
            cloud_map: true,
            light_map: true,
            soft_water: true,
            extra_animations: true,
            dynamic_lod: true,
            heat_effects: true,
            building_occlusion: true,
            show_trees: true,
            max_particles: 5000,
        }
    }
}

/// What a static detail level sets (`StaticGameLODInfo`): the fields it has in common with the
/// `Custom` branch. The rest of [`CustomLod`] is not per level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    pub texture_reduction: i32,
    pub max_particles: i32,
    pub shadow_volumes: bool,
    pub shadow_decals: bool,
    pub cloud_map: bool,
    pub light_map: bool,
    pub soft_water: bool,
    /// `UseBuildupScaffolds`, which is the same global as `ExtraAnimations`.
    pub extra_animations: bool,
    pub heat_effects: bool,
    /// `StaticGameLODInfo::m_enableDynamicLOD` (`GameLOD.h:112`): TRUE in the constructor
    /// (`GameLOD.cpp:109`) and for VeryHigh (`:272`), not in the `StaticGameLOD` field parse table
    /// (`:52-73`), and only the `Custom` branch takes it from `options.ini` (`:388`).
    pub enable_dynamic_lod: bool,
}

/// The VeryHigh preset (`GameLOD.cpp:251-273`).
const VERY_HIGH: Preset = Preset {
    texture_reduction: 0,
    max_particles: 5000,
    shadow_volumes: true,
    shadow_decals: true,
    cloud_map: true,
    light_map: true,
    soft_water: true,
    extra_animations: true,
    heat_effects: true,
    enable_dynamic_lod: true,
};

fn preset_from_block(b: &Block) -> Option<Preset> {
    let int = |k: &str| b.value(k).and_then(|v| v.trim().parse::<f32>().ok()).map(|v| v.round() as i32);
    let flag = |k: &str| b.value(k).map(parse_flag);
    Some(Preset {
        texture_reduction: int("TextureReductionFactor")?,
        max_particles: int("MaxParticleCount")?,
        shadow_volumes: flag("UseShadowVolumes")?,
        shadow_decals: flag("UseShadowDecals")?,
        cloud_map: flag("UseCloudMap")?,
        light_map: flag("UseLightMap")?,
        soft_water: flag("ShowSoftWaterEdge")?,
        extra_animations: flag("UseBuildupScaffolds")?,
        heat_effects: flag("UseHeatEffects")?,
        enable_dynamic_lod: true,
    })
}

/// One `DynamicGameLOD` block (`DynamicGameLODInfo`, `GameLOD.cpp:113-141`). The dynamic level is
/// chosen from the average FPS (`W3DDisplay.cpp:1857-1866`) and its particle gate is read by
/// `ParticleSystem::createParticle` (`ParticleSys.cpp:1894-1897`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DynamicPreset {
    /// `MinimumFPS` (`GameLOD.h:133`); the level applies while `min_fps < (Int)averageFPS`.
    pub min_fps: i32,
    /// `ParticleSkipMask` (`GameLOD.cpp:116`, `isParticleSkipped` `GameLOD.h:231-234`). 0 skips none.
    pub particle_skip_mask: u32,
    /// `MinParticlePriority`: particles below it are never created at this level.
    pub min_particle_priority: ParticlePriority,
    /// `MinParticleSkipPriority`: particles below it are created only on the skip mask's turn.
    pub min_particle_skip_priority: ParticlePriority,
}

impl Default for DynamicPreset {
    /// `DynamicGameLODInfo::DynamicGameLODInfo` (`GameLOD.cpp:133-141`).
    fn default() -> Self {
        DynamicPreset {
            min_fps: 0,
            particle_skip_mask: 0,
            min_particle_priority: ParticlePriority::WeaponExplosion,
            min_particle_skip_priority: ParticlePriority::WeaponExplosion,
        }
    }
}

/// The `DynamicGameLOD` level names (`GameLOD.cpp:124-130`), in `DynamicGameLODLevel` order.
pub const DYNAMIC_GAME_LOD_NAMES: [&str; 4] = ["Low", "Medium", "High", "VeryHigh"];

fn dynamic_preset_from_block(b: &Block) -> DynamicPreset {
    let d = DynamicPreset::default();
    let int = |k: &str| b.value(k).and_then(|v| v.trim().parse::<f32>().ok()).map(|v| v.round() as i32);
    DynamicPreset {
        min_fps: int("MinimumFPS").unwrap_or(d.min_fps),
        particle_skip_mask: int("ParticleSkipMask").map_or(d.particle_skip_mask, |v| v.max(0) as u32),
        min_particle_priority: b
            .value("MinParticlePriority")
            .and_then(ParticlePriority::parse)
            .unwrap_or(d.min_particle_priority),
        min_particle_skip_priority: b
            .value("MinParticleSkipPriority")
            .and_then(ParticlePriority::parse)
            .unwrap_or(d.min_particle_skip_priority),
    }
}

/// The detail presets. Without a `StaticGameLOD` block (no game data loaded yet) Low, Medium and
/// High are absent and the stored [`CustomLod`] applies at those levels.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct LodPresets {
    low: Option<Preset>,
    medium: Option<Preset>,
    high: Option<Preset>,
    /// `DynamicGameLOD` Low, Medium, High, VeryHigh (`GameLODManager::m_dynamicGameLODInfo`).
    dynamic: [DynamicPreset; 4],
}

impl LodPresets {
    /// Reads the `StaticGameLOD` and `DynamicGameLOD` blocks of the install (`GameLODManager::init`,
    /// `GameLOD.cpp:304-320`).
    pub fn from_ini(db: &IniDb) -> Self {
        let get = |name: &str| db.get("StaticGameLOD", name).and_then(preset_from_block);
        let mut dynamic = [DynamicPreset::default(); 4];
        for (i, name) in DYNAMIC_GAME_LOD_NAMES.iter().enumerate() {
            if let Some(b) = db.get("DynamicGameLOD", name) {
                dynamic[i] = dynamic_preset_from_block(b);
            }
        }
        LodPresets { low: get("Low"), medium: get("Medium"), high: get("High"), dynamic }
    }

    fn preset(&self, detail: Detail) -> Option<Preset> {
        match detail {
            Detail::Low => self.low,
            Detail::Medium => self.medium,
            Detail::High => self.high,
            Detail::VeryHigh => Some(VERY_HIGH),
            Detail::Custom => None,
        }
    }

    /// The values the viewer draws with at `detail`: the stored custom values, with the preset's
    /// fields replaced (`applyStaticLODLevel`). `Custom` uses the stored values as they are.
    pub fn effective(&self, detail: Detail, custom: &CustomLod) -> CustomLod {
        let mut out = *custom;
        if let Some(p) = self.preset(detail) {
            out.texture_reduction = p.texture_reduction;
            out.max_particles = p.max_particles;
            out.shadow_volumes = p.shadow_volumes;
            out.shadow_decals = p.shadow_decals;
            out.cloud_map = p.cloud_map;
            out.light_map = p.light_map;
            out.soft_water = p.soft_water;
            out.extra_animations = p.extra_animations;
            out.heat_effects = p.heat_effects;
            // `applyStaticLODLevel` (`GameLOD.cpp:586`) always copies the preset's flag; only the
            // `Custom` branch (`:388`) has already read it from `optionPref` into the global.
            out.dynamic_lod = p.enable_dynamic_lod;
        }
        out
    }

    /// The `DynamicGameLOD` block for a level index (`0` Low .. `3` VeryHigh).
    pub fn dynamic_preset(&self, level: usize) -> DynamicPreset {
        self.dynamic[level.min(3)]
    }

    /// `GameLODManager::findDynamicLODLevel` (`GameLOD.cpp:679-690`): the highest level whose
    /// `MinimumFPS` is strictly below the integer average FPS, else Low.
    pub fn dynamic_level_for_fps(&self, average_fps: f32) -> usize {
        let ifps = average_fps as i32;
        for i in (0..4).rev() {
            if self.dynamic[i].min_fps < ifps {
                return i;
            }
        }
        0
    }
}

/// The applied settings (the `GameSettings` resource) and the values the options screen edits.
///
/// Slider values use the C++ units: volumes in percent, `scroll_factor` is the keyboard scroll
/// factor times 100 (`ScrollFactor`), `gamma` is the gamma slider from 0 to 100 (50 = none).
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct GameSettings {
    pub resolution: (u32, u32),
    /// Viewer addition (retail has no fullscreen toggle here): the `Fullscreen` key.
    pub fullscreen: bool,
    /// `MusicVolume`, percent.
    pub music: i32,
    /// `SFXVolume` and `SFX3DVolume`, percent. Read as the larger of the two, written split by
    /// `Relative2DVolume` (see [`split_sfx`]).
    pub sfx: i32,
    /// `VoiceVolume`, percent.
    pub voice: i32,
    /// True while the volumes are the fallback values and the install's audio defaults have not
    /// been read. Nothing is written for them until then.
    pub volumes_pending: bool,
    /// `ScrollFactor`, the keyboard scroll factor times 100. Absent from the file, it is the install's
    /// `KeyboardDefaultScrollSpeedFactor` (see [`InstallDefaults`]).
    pub scroll_factor: i32,
    /// True while `ScrollFactor` is absent and the install's default has not been read.
    pub scroll_pending: bool,
    pub gamma: i32,
    pub language_filter: bool,
    pub alternate_mouse: bool,
    pub retaliation: bool,
    pub double_click_attack_move: bool,
    pub draw_scroll_anchor: bool,
    pub move_scroll_anchor: bool,
    pub save_camera_in_replays: bool,
    pub use_camera_in_replays: bool,
    /// The anti-aliasing combo index: 0 off, 1 2x, 2 4x, 3 8x (`AntiAliasing` holds 0, 2, 4, 8).
    /// The combo is hidden in `OptionsMenu.wnd`, so the value is kept as read and has no effect here.
    pub anti_aliasing: usize,
    /// The engine's `TextureFilter` mode index (`convert::FILTER_*`). Absent is BILINEAR (`OptionPreferences.cpp:87`);
    /// an unknown name is NONE (`TextureFilterClass::getTextureFilterMode`). Applied at launch (`options.rs`).
    pub texture_filter: u8,
    /// `AnisotropyLevel`, clamped to 2..16 (`OptionPreferences.cpp:96-99`); absent is 2x.
    pub anisotropy: u16,
    pub detail: Detail,
    pub custom: CustomLod,
    /// Keys this screen does not edit, as read, plus the typed keys' last written text.
    pub raw: BTreeMap<String, String>,
}

impl Default for GameSettings {
    /// The settings for an `options.ini` with no keys.
    fn default() -> Self {
        GameSettings::from_text("")
    }
}

/// `OptionPreferences` reads a flag as yes only for `stricmp(value, "yes") == 0` (for example
/// `getDrawScrollAnchor`, `getTreesEnabled`). Anything else is no.
fn parse_flag(v: &str) -> bool {
    v.trim().eq_ignore_ascii_case("yes")
}

/// The install's values for the defaults an absent `options.ini` key takes (`GameData`'s
/// `KeyboardDefaultScrollSpeedFactor`, `GlobalData.cpp`). Read from the install at build time; the
/// options screen seeds [`GameSettings::scroll_factor`] from it.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstallDefaults {
    /// The keyboard default scroll factor times 100.
    pub keyboard_scroll_factor: i32,
    /// `GlobalData::m_maxFieldParticleCount`, the `MaxFieldParticleCount` key of the `GameData`
    /// block (`GlobalData.cpp:402`, default 30 at `:837`).
    pub max_field_particle_count: i32,
}

/// The code default `m_keyboardDefaultScrollFactor` (`GlobalData.cpp:1020`, 0.5), used until the
/// install's value is read.
const CODE_DEFAULT_SCROLL_FACTOR: i32 = 50;

/// `GlobalData::m_maxFieldParticleCount`'s default (`GlobalData.cpp:837`).
const CODE_DEFAULT_FIELD_PARTICLES: i32 = 30;

/// `LookAtXlat.cpp` `SCROLL_MULTIPLIER`: all scrolling is scaled by 2, so the default scroll factor
/// 0.5 is a neutral 1x in the viewer's pan.
const SCROLL_MULTIPLIER: f32 = 2.0;

impl InstallDefaults {
    /// Reads `GameData`'s `KeyboardDefaultScrollSpeedFactor`. The shipped install does not set it
    /// (it sets `KeyboardScrollSpeedFactor` = 2.0, which the options screen overwrites from
    /// `ScrollFactor`, GlobalData.cpp:1210), so the code default 0.5 applies there.
    pub fn from_ini(db: &IniDb) -> InstallDefaults {
        let gamedata = db.resolved("GameData", "");
        let factor = gamedata
            .as_ref()
            .and_then(|b| b.value("KeyboardDefaultScrollSpeedFactor").and_then(|v| v.trim().parse::<f32>().ok()))
            .unwrap_or(CODE_DEFAULT_SCROLL_FACTOR as f32 / 100.0);
        let field = gamedata
            .as_ref()
            .and_then(|b| b.value("MaxFieldParticleCount").and_then(|v| v.trim().parse::<f32>().ok()))
            .map(|v| v.round() as i32)
            .unwrap_or(CODE_DEFAULT_FIELD_PARTICLES);
        InstallDefaults { keyboard_scroll_factor: ((factor * 100.0).round() as i32).max(1), max_field_particle_count: field.max(0) }
    }
}

fn yes_no(b: bool) -> String {
    (if b { "yes" } else { "no" }).to_owned()
}

/// The two stored SFX volumes for a slider value (`saveOptions`): the 2D one is scaled down by a
/// negative `Relative2DVolume`, the 3D one by a positive one.
pub fn split_sfx(sfx: i32, relative_2d: f32) -> (i32, i32) {
    let s = sfx as f32;
    let rel = relative_2d.clamp(-1.0, 1.0);
    let (two_d, three_d) = if rel < 0.0 {
        (s * (1.0 + rel), s)
    } else if rel > 0.0 {
        (s, s * (1.0 - rel))
    } else {
        (s, s)
    };
    (two_d.round() as i32, three_d.round() as i32)
}

/// The `Gamma` slider's display gamma (`saveOptions`: 0 -> 0.6, 50 -> 1.0, 100 -> 2.0).
pub fn gamma_value(slider: i32) -> f32 {
    if slider < 50 {
        if slider <= 0 {
            0.6
        } else {
            1.0 - 0.4 * (50 - slider) as f32 / 50.0
        }
    } else {
        1.0 + (slider - 50) as f32 / 50.0
    }
}

impl GameSettings {
    /// Parses `options.ini` text. A missing key takes the value `OptionPreferences` uses for it.
    pub fn from_text(text: &str) -> GameSettings {
        let mut raw = BTreeMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') || line.starts_with('[') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                raw.insert(k.trim().to_owned(), v.trim().to_owned());
            }
        }
        let int = |k: &str| raw.get(k).and_then(|v| v.parse::<f32>().ok()).map(|v| v.round() as i32);
        let flag = |k: &str, default: bool| raw.get(k).map_or(default, |v| parse_flag(v));

        let music = int("MusicVolume");
        let voice = int("VoiceVolume");
        let sfx = match (int("SFXVolume"), int("SFX3DVolume")) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        let volumes_pending = music.is_none() || voice.is_none() || sfx.is_none();

        let resolution = raw
            .get("Resolution")
            .and_then(|v| {
                let mut it = v.split_whitespace();
                let w = it.next()?.parse().ok()?;
                let h = it.next()?.parse().ok()?;
                Some((w, h))
            })
            .unwrap_or(DEFAULT_RESOLUTION);

        let custom = CustomLod {
            texture_reduction: int("TextureReduction").unwrap_or(0).clamp(0, 2),
            shadow_volumes: flag("UseShadowVolumes", true),
            shadow_decals: flag("UseShadowDecals", true),
            cloud_map: flag("UseCloudMap", true),
            light_map: flag("UseLightMap", true),
            soft_water: flag("ShowSoftWaterEdge", true),
            extra_animations: flag("ExtraAnimations", true),
            dynamic_lod: flag("DynamicLOD", true),
            heat_effects: flag("HeatEffects", true),
            building_occlusion: flag("BuildingOcclusion", true),
            show_trees: flag("ShowTrees", true),
            max_particles: int("MaxParticleCount").unwrap_or(5000).max(100),
        };

        GameSettings {
            resolution,
            fullscreen: flag("Fullscreen", false),
            music: music.unwrap_or(FALLBACK_MUSIC).clamp(0, 100),
            sfx: sfx.unwrap_or(FALLBACK_SFX).clamp(0, 100),
            voice: voice.unwrap_or(FALLBACK_VOICE).clamp(0, 100),
            volumes_pending,
            // `getScrollFactor`: at least 1; absent means `m_keyboardDefaultScrollFactor` (install value).
            scroll_factor: int("ScrollFactor").map_or(CODE_DEFAULT_SCROLL_FACTOR, |v| v.max(1)),
            scroll_pending: int("ScrollFactor").is_none(),
            gamma: int("Gamma").unwrap_or(50).clamp(0, 100),
            language_filter: raw.get("LanguageFilter").is_some_and(|v| v.as_str() == "true"),
            alternate_mouse: flag("UseAlternateMouse", false),
            retaliation: flag("Retaliation", true),
            double_click_attack_move: flag("UseDoubleClickAttackMove", false),
            draw_scroll_anchor: flag("DrawScrollAnchor", false),
            move_scroll_anchor: flag("MoveScrollAnchor", true),
            save_camera_in_replays: flag("SaveCameraInReplays", true),
            use_camera_in_replays: flag("UseCameraInReplays", true),
            anti_aliasing: match int("AntiAliasing") {
                Some(2) => 1,
                Some(4) => 2,
                Some(8) => 3,
                _ => 0,
            },
            texture_filter: raw.get("TextureFilter").map_or(FILTER_BILINEAR, |v| {
                // `TextureFilterClass::getTextureFilterMode`: the names of `TextureFilterModeString`, case-insensitive.
                ["None", "Point", "Bilinear", "Trilinear", "Anisotropic"]
                    .iter()
                    .position(|n| n.eq_ignore_ascii_case(v.as_str()))
                    .map_or(FILTER_NONE, |i| i as u8)
            }),
            anisotropy: int("AnisotropyLevel").unwrap_or(2).clamp(2, 16) as u16,
            detail: raw.get("StaticGameLOD").and_then(|v| Detail::from_key(v)).unwrap_or(Detail::VeryHigh),
            custom,
            raw,
        }
    }

    /// The `options.ini` text for these settings, merged over the keys read from the file. The
    /// volumes are left out while [`GameSettings::volumes_pending`]; the custom keys are written
    /// only at `Custom` (`saveOptions`), so a file's custom values survive a preset.
    pub fn to_text(&self, relative_2d: f32) -> String {
        let mut m = self.raw.clone();
        if !self.volumes_pending {
            let (two_d, three_d) = split_sfx(self.sfx, relative_2d);
            m.insert("MusicVolume".into(), self.music.to_string());
            m.insert("SFXVolume".into(), two_d.to_string());
            m.insert("SFX3DVolume".into(), three_d.to_string());
            m.insert("VoiceVolume".into(), self.voice.to_string());
        }
        m.insert("Resolution".into(), format!("{} {}", self.resolution.0, self.resolution.1));
        m.insert("Fullscreen".into(), yes_no(self.fullscreen));
        if !self.scroll_pending {
            m.insert("ScrollFactor".into(), self.scroll_factor.to_string());
        }
        m.insert("Gamma".into(), self.gamma.to_string());
        m.insert("LanguageFilter".into(), (if self.language_filter { "true" } else { "false" }).into());
        m.insert("UseAlternateMouse".into(), yes_no(self.alternate_mouse));
        m.insert("Retaliation".into(), yes_no(self.retaliation));
        m.insert("UseDoubleClickAttackMove".into(), yes_no(self.double_click_attack_move));
        m.insert("DrawScrollAnchor".into(), yes_no(self.draw_scroll_anchor));
        m.insert("MoveScrollAnchor".into(), yes_no(self.move_scroll_anchor));
        m.insert("SaveCameraInReplays".into(), yes_no(self.save_camera_in_replays));
        m.insert("UseCameraInReplays".into(), yes_no(self.use_camera_in_replays));
        let aa = [0, 2, 4, 8][self.anti_aliasing.min(3)];
        m.insert("AntiAliasing".into(), aa.to_string());
        m.insert("StaticGameLOD".into(), self.detail.key().to_owned());
        if self.detail == Detail::Custom {
            let c = &self.custom;
            m.insert("TextureReduction".into(), c.texture_reduction.to_string());
            m.insert("UseShadowVolumes".into(), yes_no(c.shadow_volumes));
            m.insert("UseShadowDecals".into(), yes_no(c.shadow_decals));
            m.insert("UseCloudMap".into(), yes_no(c.cloud_map));
            m.insert("UseLightMap".into(), yes_no(c.light_map));
            m.insert("ShowSoftWaterEdge".into(), yes_no(c.soft_water));
            m.insert("ExtraAnimations".into(), yes_no(c.extra_animations));
            m.insert("DynamicLOD".into(), yes_no(c.dynamic_lod));
            m.insert("HeatEffects".into(), yes_no(c.heat_effects));
            m.insert("BuildingOcclusion".into(), yes_no(c.building_occlusion));
            m.insert("ShowTrees".into(), yes_no(c.show_trees));
            m.insert("MaxParticleCount".into(), c.max_particles.to_string());
        }
        let mut out = String::new();
        for (k, v) in &m {
            out.push_str(&format!("{k} = {v}\n"));
        }
        out
    }

    /// The custom values the viewer draws with, after the detail preset.
    pub fn lod(&self, presets: &LodPresets) -> CustomLod {
        presets.effective(self.detail, &self.custom)
    }

    /// Whether the right button may scroll the view (`LookAtXlat.cpp:264`:
    /// `!UseAlternateMouse || UseRightMouseScrollWithAlternateMouse`). In alternate mouse mode the key
    /// decides, and an absent key is yes (`GlobalData.cpp:1062`, the Zero Hour value).
    pub fn right_mouse_scroll_allowed(&self) -> bool {
        !self.alternate_mouse || self.raw.get("UseRightMouseScrollWithAlternateMouse").is_none_or(|v| parse_flag(v))
    }

    /// The camera's pan multiplier: `LookAtXlat` scales the keyboard scroll factor by
    /// `SCROLL_MULTIPLIER` (2), so the default factor 0.5 gives the neutral 1.0 the viewer pans at.
    pub fn scroll_speed(&self) -> f32 {
        self.scroll_factor as f32 / 100.0 * SCROLL_MULTIPLIER
    }

    /// Takes the install's keyboard default scroll factor when `ScrollFactor` is absent from the file.
    pub fn seed_scroll(&mut self, install: &InstallDefaults) {
        if self.scroll_pending {
            self.scroll_factor = install.keyboard_scroll_factor;
            self.scroll_pending = false;
        }
    }

    /// Loads `options.ini` from the user data dir, or the defaults when it is missing.
    pub fn load() -> GameSettings {
        let Ok(dir) = userdata::ensure() else { return GameSettings::default() };
        match std::fs::read_to_string(dir.join("options.ini")) {
            Ok(text) => GameSettings::from_text(&text),
            Err(_) => GameSettings::default(),
        }
    }

    /// Writes `options.ini` to the user data dir.
    pub fn save(&self, relative_2d: f32) {
        let Ok(dir) = userdata::ensure() else { return };
        if let Err(e) = std::fs::write(dir.join("options.ini"), self.to_text(relative_2d)) {
            bevy::log::warn!("options: could not write options.ini: {e}");
        }
    }
}

/// The ACCEPT / CANCEL session. While the screen is open it edits `draft`; the applied settings are
/// not touched until [`Session::accept`]. [`Session::cancel`] drops the draft and nothing is applied
/// or written.
///
/// `OptionsMenu.cpp` does the same: BACK drops the preferences without `pref->write()`, and the
/// volumes, scroll factor, gamma, detail, resolution and anti-aliasing take effect in `saveOptions`
/// (ACCEPT) only. The viewer adds nothing live.
#[derive(Debug, Default)]
pub struct Session {
    draft: Option<GameSettings>,
}

impl Session {
    pub fn is_open(&self) -> bool {
        self.draft.is_some()
    }

    /// Starts a session from the applied settings. Opening again always starts from them.
    pub fn open(&mut self, applied: &GameSettings) {
        self.draft = Some(applied.clone());
    }

    pub fn draft(&self) -> Option<&GameSettings> {
        self.draft.as_ref()
    }

    pub fn draft_mut(&mut self) -> Option<&mut GameSettings> {
        self.draft.as_mut()
    }

    /// ACCEPT: the draft becomes the applied settings. Returns false when no session was open.
    pub fn accept(&mut self, applied: &mut GameSettings) -> bool {
        match self.draft.take() {
            Some(d) => {
                *applied = d;
                true
            }
            None => false,
        }
    }

    /// CANCEL, Esc and BACK: the draft is dropped.
    pub fn cancel(&mut self) {
        self.draft = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The values that are not the raw map, for comparing a reloaded file with the source.
    fn values(s: &GameSettings) -> GameSettings {
        let mut c = s.clone();
        c.raw.clear();
        c
    }

        fn edited() -> GameSettings {
        GameSettings {
            resolution: (1280, 720),
            fullscreen: true,
            music: 37,
            sfx: 64,
            voice: 12,
            volumes_pending: false,
            scroll_factor: 150,
            scroll_pending: false,
            gamma: 73,
            language_filter: true,
            alternate_mouse: true,
            retaliation: false,
            double_click_attack_move: true,
            draw_scroll_anchor: true,
            move_scroll_anchor: false,
            save_camera_in_replays: false,
            use_camera_in_replays: false,
            anti_aliasing: 2,
            detail: Detail::Custom,
            custom: CustomLod {
                texture_reduction: 2,
                shadow_volumes: false,
                shadow_decals: false,
                cloud_map: false,
                light_map: true,
                soft_water: false,
                extra_animations: false,
                dynamic_lod: false,
                heat_effects: false,
                building_occlusion: false,
                show_trees: false,
                max_particles: 1234,
            },
            ..GameSettings::default()
        }
    }

    #[test]
    fn texture_filter_and_anisotropy_are_read_as_the_engine_reads_them() {
        // `OptionPreferences::getTextureFilterMode` (absent: BILINEAR; names via `TextureFilterModeString`, any
        // case; unknown: NONE) and `getTextureAnisotropyLevel` (clamped to 2..16, absent: 2x).
        let s = GameSettings::from_text("TextureFilter = trilinear\nAnisotropyLevel = 8\n");
        assert_eq!((s.texture_filter, s.anisotropy), (crate::convert::FILTER_TRILINEAR, 8));
        let d = GameSettings::from_text("");
        assert_eq!((d.texture_filter, d.anisotropy), (crate::convert::FILTER_BILINEAR, 2));
        let u = GameSettings::from_text("TextureFilter = Mipmapped\nAnisotropyLevel = 99\n");
        assert_eq!((u.texture_filter, u.anisotropy), (crate::convert::FILTER_NONE, 16));
    }

    #[test]
    fn an_empty_file_gives_the_documented_defaults() {
        let s = GameSettings::default();
        assert!(s.volumes_pending);
        assert_eq!((s.music, s.sfx, s.voice), (FALLBACK_MUSIC, FALLBACK_SFX, FALLBACK_VOICE));
        assert_eq!(s.resolution, DEFAULT_RESOLUTION);
        assert_eq!(s.scroll_factor, 50, "the code default 0.5 until the install's value is read");
        assert!(s.scroll_pending);
        assert_eq!(s.gamma, 50);
        assert_eq!(s.detail, Detail::VeryHigh);
        assert_eq!(s.custom, CustomLod::default());
        assert!(!s.language_filter, "OptionsMenu.cpp: an absent LanguageFilter is unchecked");
        assert!(s.retaliation && s.move_scroll_anchor && s.use_camera_in_replays);
    }

    #[test]
    fn typed_values_survive_a_save_and_reload() {
        let s = edited();
        let text = s.to_text(-0.1);
        let back = GameSettings::from_text(&text);
        assert_eq!(values(&back), values(&s));
        // Written sorted, one per line, with the C++ separator.
        assert!(text.lines().all(|l| l.contains(" = ")));
        assert!(text.lines().collect::<Vec<_>>().windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn sfx_is_written_with_the_relative_split_and_read_back_as_the_larger() {
        let s = GameSettings { sfx: 80, volumes_pending: false, ..GameSettings::default() };
        let text = s.to_text(-0.1);
        assert!(text.contains("SFXVolume = 72\n"), "2D is scaled by Relative2DVolume -10%: {text}");
        assert!(text.contains("SFX3DVolume = 80\n"));
        assert_eq!(GameSettings::from_text(&text).sfx, 80, "getSoundVolume and get3DSoundVolume: the max");
    }

    #[test]
    fn volumes_are_not_written_while_they_are_still_the_fallback() {
        let s = GameSettings::default();
        assert!(s.volumes_pending);
        let text = s.to_text(-0.1);
        assert!(!text.contains("MusicVolume"), "the install's defaults are not known yet: {text}");
        assert!(!text.contains("SFXVolume"));
        assert!(!text.contains("VoiceVolume"));
    }

    #[test]
    fn keys_the_screen_does_not_edit_survive_a_save() {
        let file = "IPAddress = 10.0.0.2\nGameSpyIPAddress = 10.0.0.3\nFirewallPortOverride = 1234\n\
                    FirewallBehavior = 3\nUseRightMouseScrollWithAlternateMouse = no\nArchiveReplays = yes\n\
                    ; a comment\nCampaignDifficulty = 1\n";
        let s = GameSettings::from_text(file);
        let text = s.to_text(0.0);
        for line in ["IPAddress = 10.0.0.2", "GameSpyIPAddress = 10.0.0.3", "FirewallPortOverride = 1234", "FirewallBehavior = 3", "UseRightMouseScrollWithAlternateMouse = no", "ArchiveReplays = yes", "CampaignDifficulty = 1"] {
            assert!(text.contains(&format!("{line}\n")), "{line} lost: {text}");
        }
        assert!(!text.contains("; a comment"), "comments are not kept");
    }

    #[test]
    fn firewall_refresh_is_a_raw_key_written_on_accept() {
        let mut applied = GameSettings::from_text("FirewallBehavior = 3\n");
        let mut session = Session::default();
        session.open(&applied);
        session.draft_mut().unwrap().raw.insert("FirewallBehavior".into(), "0".into());
        assert!(session.accept(&mut applied));
        assert!(applied.to_text(0.0).contains("FirewallBehavior = 0\n"));
    }

    #[test]
    fn custom_keys_are_written_only_at_custom_detail() {
        let mut s = GameSettings::from_text("TextureReduction = 2\nMaxParticleCount = 900\nStaticGameLOD = High\n");
        assert_eq!(s.detail, Detail::High);
        // Edit the custom values while High is selected: nothing of them is written.
        s.custom.texture_reduction = 0;
        s.custom.max_particles = 7777;
        let text = s.to_text(0.0);
        assert!(text.contains("TextureReduction = 2\n"), "the file's custom value stays: {text}");
        assert!(text.contains("MaxParticleCount = 900\n"));
        // At Custom the edited values are written.
        s.detail = Detail::Custom;
        let text = s.to_text(0.0);
        assert!(text.contains("TextureReduction = 0\n") && text.contains("MaxParticleCount = 7777\n"), "{text}");
    }

    #[test]
    fn cancel_leaves_the_applied_settings_as_they_were() {
        let applied = edited();
        let before = applied.clone();
        let mut session = Session::default();
        session.open(&applied);
        {
            let d = session.draft_mut().expect("open");
            d.music = 0;
            d.resolution = (800, 600);
            d.detail = Detail::Low;
            d.custom.shadow_decals = true;
        }
        session.cancel();
        assert!(!session.is_open());
        assert_eq!(applied, before, "CANCEL applies nothing");
        // A later accept with no draft does nothing either.
        let mut applied = applied;
        assert!(!session.accept(&mut applied));
        assert_eq!(applied, before);
    }

    #[test]
    fn accept_applies_the_draft_and_reopening_starts_from_it() {
        let mut applied = GameSettings::default();
        let mut session = Session::default();
        session.open(&applied);
        session.draft_mut().unwrap().music = 20;
        assert_eq!(applied.music, FALLBACK_MUSIC, "not applied while editing");
        assert!(session.accept(&mut applied));
        assert_eq!(applied.music, 20);
        session.open(&applied);
        assert_eq!(session.draft().unwrap().music, 20, "a new session starts from the applied value");
    }

    #[test]
    fn detail_names_round_trip() {
        for d in Detail::ALL {
            assert_eq!(Detail::from_key(d.key()), Some(d));
            assert_eq!(Detail::from_key(&d.key().to_uppercase()), Some(d));
        }
        assert_eq!(Detail::from_key("Ultra"), None);
        assert_eq!(Detail::Custom.next(), Detail::Low, "the combo cycles");
    }

    #[test]
    fn gamma_follows_the_c_formula() {
        assert!((gamma_value(50) - 1.0).abs() < 1e-6);
        assert!((gamma_value(0) - 0.6).abs() < 1e-6);
        assert!((gamma_value(100) - 2.0).abs() < 1e-6);
        assert!((gamma_value(25) - 0.8).abs() < 1e-6);
    }

    #[test]
    fn scroll_speed_is_the_factor_times_the_lookat_multiplier() {
        // 0.5 (the default) is the neutral 1.0 pan, because LookAtXlat's SCROLL_MULTIPLIER is 2.
        let s = GameSettings::from_text("ScrollFactor = 50\n");
        assert_eq!(s.scroll_factor, 50);
        assert!((s.scroll_speed() - 1.0).abs() < 1e-6, "mutant check: a multiplier of 1 would give 0.5");
        assert!((GameSettings::from_text("ScrollFactor = 290\n").scroll_speed() - 5.8).abs() < 1e-6);
        assert_eq!(GameSettings::from_text("ScrollFactor = 0\n").scroll_factor, 1, "getScrollFactor clamps below 1");
    }

    #[test]
    fn the_install_default_seeds_only_an_absent_scroll_factor() {
        let install = InstallDefaults { keyboard_scroll_factor: 40, max_field_particle_count: 30 };
        let mut absent = GameSettings::default();
        absent.seed_scroll(&install);
        assert_eq!((absent.scroll_factor, absent.scroll_pending), (40, false));
        let mut present = GameSettings::from_text("ScrollFactor = 120\n");
        present.seed_scroll(&install);
        assert_eq!(present.scroll_factor, 120, "a key in the file is kept");
    }

    #[test]
    fn a_pending_scroll_factor_is_not_written_and_a_seeded_one_is() {
        let pending = GameSettings::default();
        assert!(!pending.to_text(0.0).contains("ScrollFactor"));
        let mut seeded = GameSettings::default();
        seeded.seed_scroll(&InstallDefaults { keyboard_scroll_factor: 50, max_field_particle_count: 30 });
        assert!(seeded.to_text(0.0).contains("ScrollFactor = 50\n"));
    }

    #[test]
    fn the_install_default_is_read_from_gamedata() {
        let mut db = IniDb::new();
        db.extend(
            zr_ini::parse(
                "GameData.ini",
                "GameData\n KeyboardDefaultScrollSpeedFactor = 0.4\n MaxFieldParticleCount = 12\nEnd\n",
            )
            .expect("parses"),
        );
        assert_eq!(
            InstallDefaults::from_ini(&db),
            InstallDefaults { keyboard_scroll_factor: 40, max_field_particle_count: 12 }
        );
        // The shipped install has no such keys: GlobalData.cpp:1020's 0.5 and :837's 30 apply.
        assert_eq!(
            InstallDefaults::from_ini(&IniDb::new()),
            InstallDefaults { keyboard_scroll_factor: 50, max_field_particle_count: 30 }
        );
    }

    #[test]
    fn right_mouse_scroll_follows_the_alternate_mouse_key() {
        // Regular mouse: the right button always scrolls (LookAtXlat userWantsRMBScroll).
        assert!(GameSettings::default().right_mouse_scroll_allowed());
        // Alternate mouse: an absent key is yes (Zero Hour GlobalData.cpp:1062), "no" turns it off.
        let alt = GameSettings::from_text("UseAlternateMouse = yes\n");
        assert!(alt.right_mouse_scroll_allowed(), "absent key is yes in Zero Hour");
        let alt_no = GameSettings::from_text("UseAlternateMouse = yes\nUseRightMouseScrollWithAlternateMouse = no\n");
        assert!(!alt_no.right_mouse_scroll_allowed(), "the key set to no disables it in alternate mouse mode");
        // The key alone does nothing in regular mode.
        assert!(GameSettings::from_text("UseRightMouseScrollWithAlternateMouse = no\n").right_mouse_scroll_allowed());
    }

    #[test]
    fn flags_are_yes_only_like_stricmp() {
        // OptionPreferences: `stricmp(value, "yes") == 0`; `true`, `1` and friends are no.
        for v in ["true", "1", "y", "ok", "no"] {
            assert!(!GameSettings::from_text(&format!("UseAlternateMouse = {v}\n")).alternate_mouse, "{v} must be no");
        }
        assert!(GameSettings::from_text("UseAlternateMouse = YES\n").alternate_mouse, "case does not matter");
        assert!(GameSettings::from_text("MoveScrollAnchor = yes\n").move_scroll_anchor);
        assert!(!GameSettings::from_text("MoveScrollAnchor = no\n").move_scroll_anchor);
    }

    fn presets_from(text: &str) -> LodPresets {
        let mut db = IniDb::new();
        db.extend(zr_ini::parse("GameLODPresets.ini", text).expect("parses"));
        LodPresets::from_ini(&db)
    }

    const LOW_BLOCK: &str = "StaticGameLOD Low\n MaxParticleCount = 500\n UseShadowVolumes = No\n UseShadowDecals = No\n\
        UseCloudMap = No\n UseLightMap = No\n ShowSoftWaterEdge = No\n UseBuildupScaffolds = No\n UseTreeSway = No\n\
        UseHeatEffects = No\n TextureReductionFactor = 1\n MinimumFPS = 10\n MinimumProcessorFps = 0\n\
        SampleCount2D = 6\n SampleCount3D = 24\n StreamCount = 2\n\
        MaxTankTrackEdges = 30\n MaxTankTrackOpaqueEdges = 15\n MaxTankTrackFadeDelay = 5000\n\
        UseEmissiveNightMaterials = No\nEnd\n";

    #[test]
    fn a_preset_replaces_only_its_own_fields() {
        let presets = presets_from(LOW_BLOCK);
        let stored = CustomLod { dynamic_lod: false, building_occlusion: false, show_trees: false, max_particles: 4000, ..CustomLod::default() };
        let low = presets.effective(Detail::Low, &stored);
        assert!(!low.shadow_decals && !low.shadow_volumes && !low.heat_effects && !low.extra_animations);
        assert_eq!((low.max_particles, low.texture_reduction), (500, 1));
        // `applyStaticLODLevel` copies `m_enableDynamicLOD` from the preset, which is TRUE for every
        // static level (`GameLOD.cpp:109`, `:586`); only Custom keeps the stored option (`:388`).
        assert!(low.dynamic_lod, "a static level always enables dynamic LOD");
        assert!(!low.building_occlusion && !low.show_trees, "not per level: the stored values stay");
        // Medium has no block here: the stored values apply.
        assert_eq!(presets.effective(Detail::Medium, &stored), stored);
        // Custom uses the stored values, whatever the presets say.
        assert_eq!(presets.effective(Detail::Custom, &stored), stored);
    }

    #[test]
    fn very_high_is_the_code_table_without_game_data() {
        let stored = CustomLod { shadow_decals: false, max_particles: 100, ..CustomLod::default() };
        let vh = LodPresets::default().effective(Detail::VeryHigh, &stored);
        assert!(vh.shadow_decals, "GameLOD.cpp: VeryHigh turns the decal shadows on");
        assert_eq!(vh.max_particles, 5000);
        assert_eq!(LodPresets::default().effective(Detail::Low, &stored), stored, "no Low block: stored values");
    }

    /// The `DynamicGameLOD` blocks' particle gate and the FPS -> level choice
    /// (`DynamicGameLODInfo`, `GameLOD.cpp:113-141`; `findDynamicLODLevel`, `GameLOD.cpp:679-690`).
    /// These are the shipped `GameLOD.ini` values.
    #[test]
    fn dynamic_lod_blocks_parse_and_pick_the_level_by_fps() {
        let mut db = IniDb::new();
        db.extend(
            zr_ini::parse(
                "GameLOD.ini",
                "DynamicGameLOD = Low\n MinimumFPS=0\n ParticleSkipMask=3\n MinParticlePriority=AREA_EFFECT\n MinParticleSkipPriority=CRITICAL\nEnd\n\
                 DynamicGameLOD = Medium\n MinimumFPS=10\n ParticleSkipMask=1\n MinParticlePriority=WEAPON_TRAIL\n MinParticleSkipPriority=CRITICAL\nEnd\n\
                 DynamicGameLOD = High\n MinimumFPS=20\n ParticleSkipMask=0\n MinParticlePriority=UNIT_DAMAGE_FX\n MinParticleSkipPriority=CRITICAL\nEnd\n\
                 DynamicGameLOD = VeryHigh\n MinimumFPS=25\n ParticleSkipMask=0\n MinParticlePriority=WEAPON_EXPLOSION\n MinParticleSkipPriority=CRITICAL\nEnd\n",
            )
            .expect("parses"),
        );
        let p = LodPresets::from_ini(&db);
        let low = p.dynamic_preset(0);
        assert_eq!(low.particle_skip_mask, 3);
        assert_eq!(low.min_particle_priority, ParticlePriority::AreaEffect);
        assert_eq!(low.min_particle_skip_priority, ParticlePriority::Critical);
        assert_eq!(p.dynamic_preset(1).min_particle_priority, ParticlePriority::WeaponTrail);
        assert_eq!(p.dynamic_preset(2).min_particle_priority, ParticlePriority::UnitDamageFx);
        // `findDynamicLODLevel` compares the integer FPS strictly: 25 is not VeryHigh (25 < 25 false).
        assert_eq!(p.dynamic_level_for_fps(60.0), 3);
        assert_eq!(p.dynamic_level_for_fps(25.0), 2);
        assert_eq!(p.dynamic_level_for_fps(20.0), 1);
        assert_eq!(p.dynamic_level_for_fps(10.0), 0);
        assert_eq!(p.dynamic_level_for_fps(0.0), 0, "no block matches: Low");
        // Missing blocks are the `DynamicGameLODInfo` constructor's zeros.
        assert_eq!(LodPresets::default().dynamic_preset(3), DynamicPreset::default());
        assert_eq!(LodPresets::default().dynamic_level_for_fps(60.0), 3, "0 < 60");
    }

    #[test]
    fn a_detail_that_is_not_in_the_file_is_very_high() {
        assert_eq!(GameSettings::from_text("StaticGameLOD = Bogus\n").detail, Detail::VeryHigh);
        assert_eq!(GameSettings::from_text("StaticGameLOD = low\n").detail, Detail::Low);
    }

    #[test]
    fn the_audio_split_matches_the_saved_volumes() {
        assert_eq!(split_sfx(80, -0.1), (72, 80));
        assert_eq!(split_sfx(80, 0.25), (80, 60));
        assert_eq!(split_sfx(80, 0.0), (80, 80));
    }
}
