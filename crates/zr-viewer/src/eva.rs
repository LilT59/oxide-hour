//! The EVA announcer: the message table, the per-side sounds and the check/priority/expiry
//! runtime.
//!
//! Ported from `GeneralsMD/.../GameClient/Eva.cpp` and `Include/GameClient/Eva.h`:
//! [`EvaMessage`] mirrors the `EvaMessage` enum (and `TheEvaMessageNames`), [`EvaDatabase`] parses
//! `Data\INI\Eva\Eva.ini` (`EvaEvent` blocks: `Priority`, `TimeBetweenChecksMS`,
//! `ExpirationTimeMS`, nested `SideSounds` with `Side` + `Sounds`), and [`EvaRuntime`] reproduces
//! `Eva::update` / `isTimeForCheck` / `messageShouldPlay` / `playMessage` /
//! `processPlayingMessages`: a check can only be queued when none exists for that message, plays
//! once the highest-priority pending check wins and nothing is currently speaking, expires if it
//! never gets to play, and is throttled by `framesBetweenChecks`.
//!
//! The triggers (`TheEva->setShouldPlay(...)` in the C++) live in the client. The sim is only read:
//! damage is read from `Object::last_damage_timestamp` (`ActiveBody`), production/upgrades from the
//! object's `ProductionState`, ranks from `GameLogic::player_rank_level`, superweapon readiness from
//! `GameLogic::special_power_ready_at` / `ParticleUplinkCannonUpdate` status. The `Eva.ini`
//! `SideSounds` select the local side's dialog, played through [`crate::audio::AudioQueue`].

use std::collections::BTreeMap;

use bevy::prelude::*;
use zr_ini::{IniDb, Item};
use zr_sim::damage::Relationship;
use zr_sim::superweapon::puc_status;
use zr_sim::template::kind_of;
use zr_sim::{ObjectId, PlayerIndex, LOGICFRAMES_PER_SECOND};

use crate::audio::{AudioBank, AudioQueue};
use crate::messages::MessageRequests;
use crate::radar::{RadarEventType, RadarEvents};
use crate::sim_view::SimRes;

/// `EVA_COUNT` / `TheEvaMessageNames`. The order is load-bearing: it is the index into the
/// `s_shouldPlayFuncs` table and the `TheEvaMessageNames` array in the C++.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(usize)]
#[allow(non_camel_case_types)]
pub enum EvaMessage {
    LowPower = 0,
    InsufficientFunds,
    SuperweaponDetected_Own_ParticleCannon,
    SuperweaponDetected_Own_Nuke,
    SuperweaponDetected_Own_ScudStorm,
    SuperweaponDetected_Ally_ParticleCannon,
    SuperweaponDetected_Ally_Nuke,
    SuperweaponDetected_Ally_ScudStorm,
    SuperweaponDetected_Enemy_ParticleCannon,
    SuperweaponDetected_Enemy_Nuke,
    SuperweaponDetected_Enemy_ScudStorm,
    SuperweaponLaunched_Own_ParticleCannon,
    SuperweaponLaunched_Own_Nuke,
    SuperweaponLaunched_Own_ScudStorm,
    SuperweaponLaunched_Ally_ParticleCannon,
    SuperweaponLaunched_Ally_Nuke,
    SuperweaponLaunched_Ally_ScudStorm,
    SuperweaponLaunched_Enemy_ParticleCannon,
    SuperweaponLaunched_Enemy_Nuke,
    SuperweaponLaunched_Enemy_ScudStorm,
    SuperweaponReady_Own_ParticleCannon,
    SuperweaponReady_Own_Nuke,
    SuperweaponReady_Own_ScudStorm,
    SuperweaponReady_Ally_ParticleCannon,
    SuperweaponReady_Ally_Nuke,
    SuperweaponReady_Ally_ScudStorm,
    SuperweaponReady_Enemy_ParticleCannon,
    SuperweaponReady_Enemy_Nuke,
    SuperweaponReady_Enemy_ScudStorm,
    BuildingLost,
    BaseUnderAttack,
    AllyUnderAttack,
    BeaconDetected,
    EnemyBlackLotusDetected,
    EnemyJarmenKellDetected,
    EnemyColonelBurtonDetected,
    OwnBlackLotusDetected,
    OwnJarmenKellDetected,
    OwnColonelBurtonDetected,
    UnitLost,
    GeneralLevelUp,
    VehicleStolen,
    BuildingStolen,
    CashStolen,
    UpgradeComplete,
    BuildingBeingStolen,
    BuildingSabotaged,
    SuperweaponLaunched_Own_GPS_Scrambler,
    SuperweaponLaunched_Ally_GPS_Scrambler,
    SuperweaponLaunched_Enemy_GPS_Scrambler,
    SuperweaponLaunched_Own_Sneak_Attack,
    SuperweaponLaunched_Ally_Sneak_Attack,
    SuperweaponLaunched_Enemy_Sneak_Attack,
}

/// `EVA_COUNT`.
pub const EVA_COUNT: usize = 53;

/// `TheEvaMessageNames` (uppercase, compared case-insensitively by `Eva::nameToMessage`).
pub const EVA_MESSAGE_NAMES: [&str; EVA_COUNT] = [
    "LOWPOWER",
    "INSUFFICIENTFUNDS",
    "SUPERWEAPONDETECTED_OWN_PARTICLECANNON",
    "SUPERWEAPONDETECTED_OWN_NUKE",
    "SUPERWEAPONDETECTED_OWN_SCUDSTORM",
    "SUPERWEAPONDETECTED_ALLY_PARTICLECANNON",
    "SUPERWEAPONDETECTED_ALLY_NUKE",
    "SUPERWEAPONDETECTED_ALLY_SCUDSTORM",
    "SUPERWEAPONDETECTED_ENEMY_PARTICLECANNON",
    "SUPERWEAPONDETECTED_ENEMY_NUKE",
    "SUPERWEAPONDETECTED_ENEMY_SCUDSTORM",
    "SUPERWEAPONLAUNCHED_OWN_PARTICLECANNON",
    "SUPERWEAPONLAUNCHED_OWN_NUKE",
    "SUPERWEAPONLAUNCHED_OWN_SCUDSTORM",
    "SUPERWEAPONLAUNCHED_ALLY_PARTICLECANNON",
    "SUPERWEAPONLAUNCHED_ALLY_NUKE",
    "SUPERWEAPONLAUNCHED_ALLY_SCUDSTORM",
    "SUPERWEAPONLAUNCHED_ENEMY_PARTICLECANNON",
    "SUPERWEAPONLAUNCHED_ENEMY_NUKE",
    "SUPERWEAPONLAUNCHED_ENEMY_SCUDSTORM",
    "SUPERWEAPONREADY_OWN_PARTICLECANNON",
    "SUPERWEAPONREADY_OWN_NUKE",
    "SUPERWEAPONREADY_OWN_SCUDSTORM",
    "SUPERWEAPONREADY_ALLY_PARTICLECANNON",
    "SUPERWEAPONREADY_ALLY_NUKE",
    "SUPERWEAPONREADY_ALLY_SCUDSTORM",
    "SUPERWEAPONREADY_ENEMY_PARTICLECANNON",
    "SUPERWEAPONREADY_ENEMY_NUKE",
    "SUPERWEAPONREADY_ENEMY_SCUDSTORM",
    "BUILDINGLOST",
    "BASEUNDERATTACK",
    "ALLYUNDERATTACK",
    "BEACONDETECTED",
    "ENEMYBLACKLOTUSDETECTED",
    "ENEMYJARMENKELLDETECTED",
    "ENEMYCOLONELBURTONDETECTED",
    "OWNBLACKLOTUSDETECTED",
    "OWNJARMENKELLDETECTED",
    "OWNCOLONELBURTONDETECTED",
    "UNITLOST",
    "GENERALLEVELUP",
    "VEHICLESTOLEN",
    "BUILDINGSTOLEN",
    "CASHSTOLEN",
    "UPGRADECOMPLETE",
    "BUILDINGBEINGSTOLEN",
    "BUILDINGSABOTAGED",
    "SUPERWEAPONLAUNCHED_OWN_GPS_SCRAMBLER",
    "SUPERWEAPONLAUNCHED_ALLY_GPS_SCRAMBLER",
    "SUPERWEAPONLAUNCHED_ENEMY_GPS_SCRAMBLER",
    "SUPERWEAPONLAUNCHED_OWN_SNEAK_ATTACK",
    "SUPERWEAPONLAUNCHED_ALLY_SNEAK_ATTACK",
    "SUPERWEAPONLAUNCHED_ENEMY_SNEAK_ATTACK",
];

/// Every variant in enum order (`EVA_FIRST .. EVA_COUNT`).
pub const ALL_MESSAGES: [EvaMessage; EVA_COUNT] = [
    EvaMessage::LowPower,
    EvaMessage::InsufficientFunds,
    EvaMessage::SuperweaponDetected_Own_ParticleCannon,
    EvaMessage::SuperweaponDetected_Own_Nuke,
    EvaMessage::SuperweaponDetected_Own_ScudStorm,
    EvaMessage::SuperweaponDetected_Ally_ParticleCannon,
    EvaMessage::SuperweaponDetected_Ally_Nuke,
    EvaMessage::SuperweaponDetected_Ally_ScudStorm,
    EvaMessage::SuperweaponDetected_Enemy_ParticleCannon,
    EvaMessage::SuperweaponDetected_Enemy_Nuke,
    EvaMessage::SuperweaponDetected_Enemy_ScudStorm,
    EvaMessage::SuperweaponLaunched_Own_ParticleCannon,
    EvaMessage::SuperweaponLaunched_Own_Nuke,
    EvaMessage::SuperweaponLaunched_Own_ScudStorm,
    EvaMessage::SuperweaponLaunched_Ally_ParticleCannon,
    EvaMessage::SuperweaponLaunched_Ally_Nuke,
    EvaMessage::SuperweaponLaunched_Ally_ScudStorm,
    EvaMessage::SuperweaponLaunched_Enemy_ParticleCannon,
    EvaMessage::SuperweaponLaunched_Enemy_Nuke,
    EvaMessage::SuperweaponLaunched_Enemy_ScudStorm,
    EvaMessage::SuperweaponReady_Own_ParticleCannon,
    EvaMessage::SuperweaponReady_Own_Nuke,
    EvaMessage::SuperweaponReady_Own_ScudStorm,
    EvaMessage::SuperweaponReady_Ally_ParticleCannon,
    EvaMessage::SuperweaponReady_Ally_Nuke,
    EvaMessage::SuperweaponReady_Ally_ScudStorm,
    EvaMessage::SuperweaponReady_Enemy_ParticleCannon,
    EvaMessage::SuperweaponReady_Enemy_Nuke,
    EvaMessage::SuperweaponReady_Enemy_ScudStorm,
    EvaMessage::BuildingLost,
    EvaMessage::BaseUnderAttack,
    EvaMessage::AllyUnderAttack,
    EvaMessage::BeaconDetected,
    EvaMessage::EnemyBlackLotusDetected,
    EvaMessage::EnemyJarmenKellDetected,
    EvaMessage::EnemyColonelBurtonDetected,
    EvaMessage::OwnBlackLotusDetected,
    EvaMessage::OwnJarmenKellDetected,
    EvaMessage::OwnColonelBurtonDetected,
    EvaMessage::UnitLost,
    EvaMessage::GeneralLevelUp,
    EvaMessage::VehicleStolen,
    EvaMessage::BuildingStolen,
    EvaMessage::CashStolen,
    EvaMessage::UpgradeComplete,
    EvaMessage::BuildingBeingStolen,
    EvaMessage::BuildingSabotaged,
    EvaMessage::SuperweaponLaunched_Own_GPS_Scrambler,
    EvaMessage::SuperweaponLaunched_Ally_GPS_Scrambler,
    EvaMessage::SuperweaponLaunched_Enemy_GPS_Scrambler,
    EvaMessage::SuperweaponLaunched_Own_Sneak_Attack,
    EvaMessage::SuperweaponLaunched_Ally_Sneak_Attack,
    EvaMessage::SuperweaponLaunched_Enemy_Sneak_Attack,
];

impl EvaMessage {
    /// `Eva::nameToMessage`: case-insensitive lookup in `TheEvaMessageNames`.
    pub fn from_name(name: &str) -> Option<EvaMessage> {
        let idx = EVA_MESSAGE_NAMES.iter().position(|n| n.eq_ignore_ascii_case(name))?;
        Some(ALL_MESSAGES[idx])
    }

    pub fn name(self) -> &'static str {
        EVA_MESSAGE_NAMES[self as usize]
    }
}

/// One `SideSounds` block (`EvaSideSounds`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvaSideSounds {
    pub side: String,
    pub sound_names: Vec<String>,
}

/// `EvaCheckInfo`: one `EvaEvent`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvaCheckInfo {
    pub message: EvaMessage,
    pub priority: u32,
    pub frames_between_checks: u32,
    pub frames_to_expire: u32,
    pub side_sounds: Vec<EvaSideSounds>,
}

impl EvaCheckInfo {
    /// `EvaCheckInfo::EvaCheckInfo`: `Priority 1`, `TimeBetweenChecksMS 900`, `ExpirationTimeMS 150`.
    pub fn new(message: EvaMessage) -> EvaCheckInfo {
        EvaCheckInfo { message, priority: 1, frames_between_checks: 900, frames_to_expire: 150, side_sounds: Vec::new() }
    }
}

/// Every parsed `EvaEvent`, keyed by message.
#[derive(Clone, Debug, Default)]
pub struct EvaDatabase {
    infos: BTreeMap<EvaMessage, EvaCheckInfo>,
}

impl EvaDatabase {
    /// `Eva::init`'s `ini.loadFileDirectory("Data\\INI\\Eva", ...)`, from the merged [`IniDb`].
    pub fn from_ini(db: &IniDb) -> EvaDatabase {
        let mut out = EvaDatabase::default();
        for block in db.iter().filter(|b| b.kind == "EvaEvent") {
            let Some(msg) = block.name().and_then(EvaMessage::from_name) else { continue };
            let mut info = EvaCheckInfo::new(msg);
            if let Some(v) = block.value("Priority") {
                info.priority = v.trim().parse().unwrap_or(info.priority);
            }
            if let Some(v) = block.value("TimeBetweenChecksMS") {
                info.frames_between_checks = ms_to_frames(v.trim().parse().unwrap_or(0));
            }
            if let Some(v) = block.value("ExpirationTimeMS") {
                info.frames_to_expire = ms_to_frames(v.trim().parse().unwrap_or(0));
            }
            for item in &block.items {
                if let Item::Block(sb) = item {
                    if sb.kind != "SideSounds" {
                        continue;
                    }
                    let side = sb.value("Side").unwrap_or("").trim().to_owned();
                    let sound_names = sb
                        .value("Sounds")
                        .map(|s| s.split_whitespace().map(str::to_owned).collect())
                        .unwrap_or_default();
                    info.side_sounds.push(EvaSideSounds { side, sound_names });
                }
            }
            out.infos.insert(msg, info);
        }
        out
    }

    pub fn get(&self, msg: EvaMessage) -> Option<&EvaCheckInfo> {
        self.infos.get(&msg)
    }

    pub fn len(&self) -> usize {
        self.infos.len()
    }

    /// `processPlayingMessages`'s side lookup + `GameClientRandomValue` sound pick: the local
    /// side's `Sounds` list, one chosen at random (`rng(lo, hi)` inclusive).
    pub fn side_sound(&self, msg: EvaMessage, side: &str, rng: &mut impl FnMut(u32, u32) -> u32) -> Option<String> {
        let info = self.infos.get(&msg)?;
        let ss = info.side_sounds.iter().find(|s| s.side.eq_ignore_ascii_case(side))?;
        if ss.sound_names.is_empty() {
            return None;
        }
        let idx = rng(0, ss.sound_names.len() as u32 - 1) as usize;
        ss.sound_names.get(idx).cloned()
    }
}

/// `ConvertDurationFromMsecsToFrames` + `ceil` (`INI::parseDurationUnsignedInt`).
pub fn ms_to_frames(ms: u32) -> u32 {
    ((ms as f32) * (LOGICFRAMES_PER_SECOND as f32 / 1000.0)).ceil() as u32
}

/// `EvaCheck`: one queued/playing message.
#[derive(Clone, Debug, PartialEq, Eq)]
struct EvaCheck {
    message: EvaMessage,
    triggered_on_frame: u32,
    time_for_next_check: u32,
    already_played: bool,
}

/// `Eva`'s check list and flags, minus the audio handle (the client owns that).
#[derive(Clone, Debug)]
pub struct EvaRuntime {
    checks: Vec<EvaCheck>,
    should_play: [bool; EVA_COUNT],
    /// `Eva::m_previousBuildingCount` / `m_previousUnitCount` (kept for fidelity; no shipped check
    /// reads them).
    #[allow(dead_code)]
    pub previous_building_count: i32,
    #[allow(dead_code)]
    pub previous_unit_count: i32,
}

impl Default for EvaRuntime {
    fn default() -> Self {
        EvaRuntime { checks: Vec::new(), should_play: [false; EVA_COUNT], previous_building_count: 0, previous_unit_count: 0 }
    }
}

impl EvaRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    /// `Eva::setShouldPlay`.
    pub fn set_should_play(&mut self, msg: EvaMessage) {
        self.should_play[msg as usize] = true;
    }

    /// `Eva::isTimeForCheck`: false while a check for this message is pending.
    fn is_time_for_check(&self, msg: EvaMessage) -> bool {
        !self.checks.iter().any(|c| c.message == msg)
    }

    /// `Eva::messageShouldPlay`: the low-power special case, else the generic
    /// `shouldPlayGenericHandler` (consume the flag).
    fn message_should_play(&mut self, msg: EvaMessage, low_power: bool) -> bool {
        if msg == EvaMessage::LowPower {
            return low_power;
        }
        if self.should_play[msg as usize] {
            self.should_play[msg as usize] = false;
            true
        } else {
            false
        }
    }

    /// `Eva::playMessage`.
    fn play_message(&mut self, msg: EvaMessage, frame: u32, db: &EvaDatabase) {
        let Some(info) = db.get(msg) else { return };
        self.checks.push(EvaCheck {
            message: msg,
            triggered_on_frame: frame,
            time_for_next_check: frame + info.frames_between_checks,
            already_played: false,
        });
    }

    /// `Eva::processPlayingMessages`: prune, then play the highest-priority pending check if
    /// nothing is speaking. Returns the chosen message.
    fn process_playing_messages(&mut self, frame: u32, speech_playing: bool, db: &EvaDatabase) -> Option<EvaMessage> {
        self.checks.retain(|c| {
            let expire = db.get(c.message).map_or(0, |i| i.frames_to_expire);
            // Available next frame because it played, or expired without playing.
            !((c.already_played && c.time_for_next_check <= frame + 1) || (!c.already_played && c.triggered_on_frame + expire <= frame))
        });
        if self.checks.is_empty() || speech_playing {
            return None;
        }
        let mut best: Option<usize> = None;
        let mut highest = 0u32;
        for (i, c) in self.checks.iter().enumerate() {
            if c.already_played {
                continue;
            }
            let prio = db.get(c.message).map_or(0, |x| x.priority);
            if prio > highest {
                highest = prio;
                best = Some(i);
            }
        }
        let i = best?;
        let msg = self.checks[i].message;
        self.checks[i].already_played = true;
        self.checks[i].time_for_next_check = frame + db.get(msg).map_or(0, |x| x.frames_between_checks);
        Some(msg)
    }

    /// `Eva::update`: the per-frame check loop, then the play pass, then the flag reset. Returns
    /// the message to speak this frame (at most one).
    pub fn update(&mut self, frame: u32, low_power: bool, speech_playing: bool, db: &EvaDatabase) -> Option<EvaMessage> {
        if frame < 2 {
            return None;
        }
        for msg in ALL_MESSAGES {
            if self.is_time_for_check(msg) && self.message_should_play(msg, low_power) {
                self.play_message(msg, frame, db);
            }
        }
        let played = self.process_playing_messages(frame, speech_playing, db);
        // `Eva::update`: clear flags that were not probed this frame.
        self.should_play = [false; EVA_COUNT];
        played
    }
}

/// Client systems request an EVA message here; [`eva_update`] drains it.
#[derive(Resource, Default)]
pub struct EvaRequests(pub Vec<EvaMessage>);

impl EvaRequests {
    pub fn push(&mut self, msg: EvaMessage) {
        self.0.push(msg);
    }
}

/// Superweapon category for the detected/ready/launched message triples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SuperweaponKind {
    ParticleCannon,
    Nuke,
    ScudStorm,
}

/// `(prefix, kind, ally?)` -> the `EvaMessage` (the non-own branches).
fn superweapon_message(prefix: &str, kind: SuperweaponKind, rel: Relationship) -> EvaMessage {
    use EvaMessage::*;
    use SuperweaponKind::*;
    // `Relationship` is Enemies / Allies / Neutral; retail treats non-enemies as allies.
    let ally = !matches!(rel, Relationship::Enemies);
    match (prefix, kind, ally) {
        ("detected", ParticleCannon, false) => SuperweaponDetected_Enemy_ParticleCannon,
        ("detected", ParticleCannon, true) => SuperweaponDetected_Ally_ParticleCannon,
        ("detected", Nuke, false) => SuperweaponDetected_Enemy_Nuke,
        ("detected", Nuke, true) => SuperweaponDetected_Ally_Nuke,
        ("detected", ScudStorm, false) => SuperweaponDetected_Enemy_ScudStorm,
        ("detected", ScudStorm, true) => SuperweaponDetected_Ally_ScudStorm,
        ("ready", ParticleCannon, false) => SuperweaponReady_Enemy_ParticleCannon,
        ("ready", ParticleCannon, true) => SuperweaponReady_Ally_ParticleCannon,
        ("ready", Nuke, false) => SuperweaponReady_Enemy_Nuke,
        ("ready", Nuke, true) => SuperweaponReady_Ally_Nuke,
        ("ready", ScudStorm, false) => SuperweaponReady_Enemy_ScudStorm,
        ("ready", ScudStorm, true) => SuperweaponReady_Ally_ScudStorm,
        ("launched", ParticleCannon, false) => SuperweaponLaunched_Enemy_ParticleCannon,
        ("launched", ParticleCannon, true) => SuperweaponLaunched_Ally_ParticleCannon,
        ("launched", Nuke, false) => SuperweaponLaunched_Enemy_Nuke,
        ("launched", Nuke, true) => SuperweaponLaunched_Ally_Nuke,
        ("launched", ScudStorm, false) => SuperweaponLaunched_Enemy_ScudStorm,
        ("launched", ScudStorm, true) => SuperweaponLaunched_Ally_ScudStorm,
        _ => SuperweaponDetected_Ally_ParticleCannon,
    }
}

/// The `(prefix, kind)` -> own-player variant (the C++ `localPlayer == controllingPlayer` branch).
fn superweapon_message_own(prefix: &str, kind: SuperweaponKind) -> EvaMessage {
    use EvaMessage::*;
    use SuperweaponKind::*;
    match (prefix, kind) {
        ("detected", ParticleCannon) => SuperweaponDetected_Own_ParticleCannon,
        ("detected", Nuke) => SuperweaponDetected_Own_Nuke,
        ("detected", ScudStorm) => SuperweaponDetected_Own_ScudStorm,
        ("ready", ParticleCannon) => SuperweaponReady_Own_ParticleCannon,
        ("ready", Nuke) => SuperweaponReady_Own_Nuke,
        ("ready", ScudStorm) => SuperweaponReady_Own_ScudStorm,
        ("launched", ParticleCannon) => SuperweaponLaunched_Own_ParticleCannon,
        ("launched", Nuke) => SuperweaponLaunched_Own_Nuke,
        ("launched", ScudStorm) => SuperweaponLaunched_Own_ScudStorm,
        _ => SuperweaponDetected_Own_ParticleCannon,
    }
}

/// One object's previous-frame state for the trigger deltas.
#[derive(Clone, Default)]
struct ObjTrack {
    owner: PlayerIndex,
    template: String,
    structure: bool,
    victory: bool,
    harvester: bool,
    under_construction: bool,
    queue: Vec<QueueItem>,
    puc_status: u8,
    /// `(power, kind)` pairs for the superweapons this object carries.
    superweapons: Vec<(String, SuperweaponKind)>,
    damage_frame: u32,
}

#[derive(Clone, PartialEq, Eq)]
enum QueueItem {
    Unit(String),
    Upgrade(String),
}

/// Per `(object, power)` cooldown tracking for the ready/launched messages.
#[derive(Clone, Default)]
struct PowerTrack {
    on_cooldown: bool,
    ready_played: bool,
}

/// The previous-frame snapshot the trigger deltas compare against.
#[derive(Default)]
struct EvaTracker {
    objects: BTreeMap<ObjectId, ObjTrack>,
    rank: i32,
    power_state: BTreeMap<(ObjectId, String), PowerTrack>,
    initialized: bool,
}

/// The viewer's EVA state: the parsed database, the runtime and the trigger tracker.
#[derive(Resource)]
pub struct EvaState {
    pub db: EvaDatabase,
    pub runtime: EvaRuntime,
    tracker: EvaTracker,
    rng: u32,
    /// The event name of the currently-speaking EVA sound (gate for `isCurrentlyPlaying`).
    speech_event: Option<String>,
    /// `ZR_EVA_DEMO` injected once.
    demo_done: bool,
}

impl EvaState {
    pub fn new(db: EvaDatabase) -> EvaState {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x1234_5678, |d| d.as_nanos() as u32);
        EvaState { db, runtime: EvaRuntime::new(), tracker: EvaTracker::default(), rng: seed | 1, speech_event: None, demo_done: false }
    }
}

/// `ZR_EVA_LOG`: log every EVA message that plays.
fn log_enabled() -> bool {
    std::env::var_os("ZR_EVA_LOG").is_some()
}

/// Builds the EVA state from the merged INI (called by the world build).
pub fn build(ini: &IniDb) -> EvaState {
    let state = EvaState::new(EvaDatabase::from_ini(ini));
    info!("eva: {} EvaEvent messages parsed", state.db.len());
    state
}

pub fn add_systems(app: &mut App) {
    app.init_resource::<EvaRequests>();
    app.add_systems(
        Update,
        eva_update.after(crate::sim_view::sim_step).run_if(in_state(crate::shell::ShellState::InGame)),
    );
}

/// One `Eva::update` plus the client-side `setShouldPlay` triggers.
fn eva_update(
    state: Option<ResMut<EvaState>>,
    mut requests: ResMut<EvaRequests>,
    mut messages: ResMut<MessageRequests>,
    mut radar: ResMut<RadarEvents>,
    mut audio: ResMut<AudioQueue>,
    bank: Option<Res<AudioBank>>,
    sim: Option<NonSend<SimRes>>,
) {
    let (Some(mut state), Some(sim), Some(bank)) = (state, sim.as_ref(), bank.as_ref()) else { return };
    let frame = sim.game.frame();
    let local = sim.local;

    // Drain client requests (`TheEva->setShouldPlay` from the control bar / placement).
    for msg in requests.0.drain(..) {
        state.runtime.set_should_play(msg);
    }

    poll_triggers(&mut state, sim, &mut messages, &mut radar, frame, local);

    // Debug screenshot aid: inject an EVA message + text line + radar marker once.
    if !state.demo_done && frame >= 30 {
        if let Ok(demo) = std::env::var("ZR_EVA_DEMO") {
            if !demo.is_empty() {
                state.demo_done = true;
                let msg = if demo == "1" { EvaMessage::InsufficientFunds } else { EvaMessage::from_name(&demo).unwrap_or(EvaMessage::InsufficientFunds) };
                state.runtime.set_should_play(msg);
                let label = match msg {
                    EvaMessage::InsufficientFunds => "GUI:NotEnoughMoneyToBuild",
                    EvaMessage::LowPower => "GUI:LowPower",
                    EvaMessage::BaseUnderAttack => "RADAR:StructureUnderAttack",
                    _ => "GUI:NotEnoughMoneyToBuild",
                };
                messages.push_label(sim, label);
                if let Some(sp) = sim.game.spawned().iter().find(|s| s.player == local) {
                    radar.create_event(sp.pos, RadarEventType::UnderAttack, frame);
                }
                // A second, bright marker at the map centre so the radar event is unambiguous in a
                // screenshot regardless of the local shroud.
                let (w, h) = sim.game.logic().terrain().world_size();
                radar.create_event(zr_sim::Coord3D::new(w * 0.5, h * 0.5, 0.0), RadarEventType::Information, frame);
                info!("eva: demo injected {}", msg.name());
            }
        }
    }

    let low_power = !sim.game.logic().has_sufficient_power(local);
    let speech_playing = state.speech_event.as_deref().is_some_and(|e| bank.is_playing(e));
    let played = {
        // Split the field borrows so the runtime can read the database.
        let EvaState { db, runtime, .. } = &mut *state;
        runtime.update(frame, low_power, speech_playing, db)
    };
    if let Some(msg) = played {
        let side = sim.game.player(local).map_or(String::new(), |p| p.side.clone());
        let mut rng = state.rng;
        let event = state.db.side_sound(msg, &side, &mut |lo, hi| {
            rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            if hi <= lo {
                lo
            } else {
                lo + (rng >> 8) % (hi - lo + 1)
            }
        });
        state.rng = rng;
        // `GameAudio.cpp:386`: empty names and `NoSound` are culled.
        let event = event.filter(|e| !e.is_empty() && e != "NoSound");
        if let Some(ev) = event {
            if log_enabled() {
                info!("eva: {} -> {ev} (frame {frame})", msg.name());
            }
            audio.push(ev.clone(), None, None);
            state.speech_event = Some(ev);
        } else {
            state.speech_event = None;
            if log_enabled() {
                info!("eva: {} -> NoSound (frame {frame})", msg.name());
            }
        }
    }
}

/// The `setShouldPlay` triggers, read from sim state and previous-frame deltas.
fn poll_triggers(
    state: &mut EvaState,
    sim: &SimRes,
    messages: &mut MessageRequests,
    radar: &mut RadarEvents,
    frame: u32,
    local: PlayerIndex,
) {
    let logic = sim.game.logic();
    let mut current: BTreeMap<ObjectId, ObjTrack> = BTreeMap::new();
    for o in logic.objects() {
        let template = o.template.name.clone();
        let structure = o.template.is_kind_of(kind_of::STRUCTURE);
        let victory = o.template.is_kind_of(kind_of::MP_COUNT_FOR_VICTORY);
        let harvester = o.template.is_kind_of(kind_of::HARVESTER);
        let superweapons: Vec<(String, SuperweaponKind)> = o
            .template
            .special_powers
            .iter()
            .filter_map(|sp| superweapon_kind(logic.special_power_kind(&sp.power)).map(|k| (sp.power.clone(), k)))
            .collect();
        let queue = o
            .production
            .as_ref()
            .map(|p| {
                p.queue
                    .iter()
                    .map(|e| match &e.kind {
                        zr_sim::production::ProductionKind::Unit(t) => QueueItem::Unit(t.name.clone()),
                        zr_sim::production::ProductionKind::Upgrade(t) => QueueItem::Upgrade(t.clone()),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let puc_status = o.superweapon.puc.as_ref().map_or(0, |p| p.status);
        current.insert(
            o.id,
            ObjTrack {
                owner: o.player,
                template,
                structure,
                victory,
                harvester,
                under_construction: o.under_construction,
                queue,
                puc_status,
                superweapons,
                damage_frame: o.last_damage_timestamp,
            },
        );
    }

    // First frame: snapshot only, so pre-existing objects do not fire transitions.
    if !state.tracker.initialized {
        state.tracker.rank = logic.player_rank_level(local);
        state.tracker.objects = current;
        state.tracker.initialized = true;
        return;
    }

    let prev = std::mem::take(&mut state.tracker.objects);

    // Deaths (`Object.cpp`: local structure -> BuildingLost, local unit -> UnitLost).
    for (id, old) in &prev {
        if current.contains_key(id) {
            continue;
        }
        if old.owner == local {
            state.runtime.set_should_play(if old.structure { EvaMessage::BuildingLost } else { EvaMessage::UnitLost });
        }
    }

    // New / changed objects.
    for (id, now) in &current {
        let Some(old) = prev.get(id) else { continue };
        // Construction completed (`DozerAIUpdate.cpp:614`): message + radar construction event.
        if old.under_construction && !now.under_construction && now.owner == local {
            let display = sim.game.display_name(&now.template).unwrap_or(now.template.as_str()).to_owned();
            messages.push_formatted(sim, "DOZER:ConstructionComplete", &display);
            if let Some(o) = logic.find_object(*id) {
                radar.create_event(o.position, RadarEventType::Construction, frame);
            }
            // `Player::onStructureProduced`: superweapon detected.
            for (_, kind) in &now.superweapons {
                let rel = logic.relationship(local, now.owner);
                let msg = if now.owner == local { superweapon_message_own("detected", *kind) } else { superweapon_message("detected", *kind, rel) };
                state.runtime.set_should_play(msg);
            }
        }

        // Upgrades that left the production queue and are now owned (`ProductionUpdate.cpp:850`).
        for item in &old.queue {
            if now.queue.contains(item) {
                continue;
            }
            if let QueueItem::Upgrade(name) = item {
                let owned = logic.player_upgrades(now.owner).any(|u| u == name);
                if owned && now.owner == local {
                    messages.push_formatted(sim, "UPGRADE:UpgradeComplete", name);
                    if let Some(o) = logic.find_object(*id) {
                        radar.create_event(o.position, RadarEventType::Upgrade, frame);
                    }
                    state.runtime.set_should_play(EvaMessage::UpgradeComplete);
                }
            }
        }

        // Under attack (`Object.cpp:1925`: only a *locally controlled* object does the radar/EVA
        // feedback; `ActiveBody::m_lastDamageTimestamp`).
        if now.owner == local && now.damage_frame > old.damage_frame && now.damage_frame > 0 {
            under_attack(state, sim, messages, radar, id, now, frame);
        }

        // Superweapon ready / launched.
        poll_superweapon(state, sim, id, now, old, local, frame);
    }

    // General level up (`Player.cpp:2777`): the local rank rose.
    let rank = logic.player_rank_level(local);
    if rank > state.tracker.rank {
        state.runtime.set_should_play(EvaMessage::GeneralLevelUp);
    }
    state.tracker.rank = rank;
    state.tracker.objects = current;
}

/// `Radar::tryUnderAttackEvent`'s EVA/message branch for one damaged local object.
fn under_attack(
    state: &mut EvaState,
    sim: &SimRes,
    messages: &mut MessageRequests,
    radar: &mut RadarEvents,
    id: &ObjectId,
    now: &ObjTrack,
    frame: u32,
) {
    let logic = sim.game.logic();
    let Some(o) = logic.find_object(*id) else { return };
    let pos = o.position;
    // `Radar::tryUnderAttackEvent`: the message/EVA feedback only happens when the radar event is
    // actually created (map-wide suppression for under-attack, 10 s).
    if !radar.try_event(RadarEventType::UnderAttack, pos, frame) {
        return;
    }
    if now.structure && now.victory {
        state.runtime.set_should_play(EvaMessage::BaseUnderAttack);
        messages.push_label(sim, "RADAR:StructureUnderAttack");
    } else if now.harvester {
        messages.push_label(sim, "RADAR:HarvesterUnderAttack");
    } else if now.structure || o.template.is_kind_of(kind_of::INFANTRY) || o.template.is_kind_of(kind_of::VEHICLE) {
        messages.push_label(sim, "RADAR:UnitUnderAttack");
    } else {
        messages.push_label(sim, "RADAR:UnderAttack");
    }
}

/// Ready/launched for each superweapon power on a completed object.
fn poll_superweapon(state: &mut EvaState, sim: &SimRes, id: &ObjectId, now: &ObjTrack, old: &ObjTrack, local: PlayerIndex, frame: u32) {
    let logic = sim.game.logic();
    if now.under_construction || now.superweapons.is_empty() {
        return;
    }
    let rel = logic.relationship(local, now.owner);
    for (power, kind) in &now.superweapons {
        // Particle Uplink uses its own charge status (`ParticleUplinkCannonUpdate`).
        let is_puc = matches!(kind, SuperweaponKind::ParticleCannon);
        let ready_now = if is_puc {
            now.puc_status == puc_status::READY_TO_FIRE
        } else {
            logic.special_power_ready_at(*id, power) <= frame
        };
        let on_cooldown = !ready_now;
        let track = state.tracker.power_state.entry((*id, power.clone())).or_default();
        let was_on_cooldown = track.on_cooldown;
        // Launched: the cooldown just started, or the PUC entered firing.
        let puc_firing = is_puc && now.puc_status == puc_status::FIRING && old.puc_status != puc_status::FIRING;
        if (!was_on_cooldown && on_cooldown) || puc_firing {
            let msg = if now.owner == local { superweapon_message_own("launched", *kind) } else { superweapon_message("launched", *kind, rel) };
            state.runtime.set_should_play(msg);
        }
        // Ready: was on cooldown and became ready (`m_evaReadyPlayed`).
        if !ready_now {
            track.ready_played = false;
        } else if was_on_cooldown && !track.ready_played {
            let msg = if now.owner == local { superweapon_message_own("ready", *kind) } else { superweapon_message("ready", *kind, rel) };
            state.runtime.set_should_play(msg);
            track.ready_played = true;
        }
        track.on_cooldown = on_cooldown;
    }
}

fn superweapon_kind(kind: &str) -> Option<SuperweaponKind> {
    if kind.contains("PARTICLE_UPLINK_CANNON") {
        Some(SuperweaponKind::ParticleCannon)
    } else if kind.contains("NEUTRON_MISSILE") {
        Some(SuperweaponKind::Nuke)
    } else if kind == "SPECIAL_SCUD_STORM" {
        Some(SuperweaponKind::ScudStorm)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zr_ini::{Block, Field};

    fn db() -> EvaDatabase {
        // Synthetic EvaEvent blocks, mirroring the shipped file's shape.
        let mut ini = IniDb::new();
        let mut low = Block { kind: "EvaEvent".into(), header: vec!["LowPower".into()], items: Vec::new(), line: 0 };
        low.items.push(Item::Field(Field { key: "Priority".into(), value: "2".into(), line: 0 }));
        low.items.push(Item::Field(Field { key: "TimeBetweenChecksMS".into(), value: "120000".into(), line: 0 }));
        low.items.push(Item::Field(Field { key: "ExpirationTimeMS".into(), value: "5000".into(), line: 0 }));
        let mut ss = Block { kind: "SideSounds".into(), header: Vec::new(), items: Vec::new(), line: 0 };
        ss.items.push(Item::Field(Field { key: "Side".into(), value: "America".into(), line: 0 }));
        ss.items.push(Item::Field(Field { key: "Sounds".into(), value: "EvaUSA_LowPower".into(), line: 0 }));
        low.items.push(Item::Block(ss));
        ini.extend([low]);

        let mut funds = Block { kind: "EvaEvent".into(), header: vec!["InsufficientFunds".into()], items: Vec::new(), line: 0 };
        funds.items.push(Item::Field(Field { key: "Priority".into(), value: "3".into(), line: 0 }));
        funds.items.push(Item::Field(Field { key: "TimeBetweenChecksMS".into(), value: "0".into(), line: 0 }));
        ini.extend([funds]);
        EvaDatabase::from_ini(&ini)
    }

    #[test]
    fn parses_priority_and_ms_durations() {
        let d = db();
        let low = d.get(EvaMessage::LowPower).unwrap();
        assert_eq!(low.priority, 2);
        assert_eq!(low.frames_between_checks, 3600);
        assert_eq!(low.frames_to_expire, 150);
        assert_eq!(low.side_sounds[0].side, "America");
        assert_eq!(low.side_sounds[0].sound_names, vec!["EvaUSA_LowPower"]);
        assert_eq!(d.get(EvaMessage::InsufficientFunds).unwrap().frames_between_checks, 0);
    }

    #[test]
    fn name_lookup_is_case_insensitive() {
        assert_eq!(EvaMessage::from_name("lowpower"), Some(EvaMessage::LowPower));
        assert_eq!(EvaMessage::from_name("SuperweaponReady_Own_Nuke"), Some(EvaMessage::SuperweaponReady_Own_Nuke));
        assert_eq!(EvaMessage::from_name("nope"), None);
        assert_eq!(EvaMessage::LowPower.name(), "LOWPOWER");
        assert_eq!(ALL_MESSAGES.len(), EVA_COUNT);
    }

    #[test]
    fn side_sound_picks_the_local_side() {
        let d = db();
        let mut rng = |lo, _hi| lo;
        assert_eq!(d.side_sound(EvaMessage::LowPower, "america", &mut rng), Some("EvaUSA_LowPower".to_owned()));
        assert_eq!(d.side_sound(EvaMessage::LowPower, "China", &mut rng), None);
    }

    #[test]
    fn low_power_is_condition_driven() {
        let d = db();
        let mut rt = EvaRuntime::new();
        // No power -> queued immediately.
        assert_eq!(rt.update(2, true, false, &d), Some(EvaMessage::LowPower));
        // Still in m_checks, so the next frame does not queue again.
        assert_eq!(rt.update(3, true, false, &d), None);
        // Full power -> nothing queued.
        let mut rt2 = EvaRuntime::new();
        assert_eq!(rt2.update(2, false, false, &d), None);
    }

    #[test]
    fn priority_wins_over_insertion_order() {
        let d = db();
        let mut rt = EvaRuntime::new();
        rt.set_should_play(EvaMessage::InsufficientFunds); // priority 3
        rt.set_should_play(EvaMessage::LowPower); // priority 2
        assert_eq!(rt.update(2, true, false, &d), Some(EvaMessage::InsufficientFunds));
    }

    #[test]
    fn speech_playing_defers_the_play() {
        let d = db();
        let mut rt = EvaRuntime::new();
        rt.set_should_play(EvaMessage::InsufficientFunds);
        // The check is queued but cannot play while a speech is active.
        assert_eq!(rt.update(2, false, true, &d), None);
        // Once the speech finishes the queued check plays (the flag was consumed at frame 2).
        assert_eq!(rt.update(3, false, false, &d), Some(EvaMessage::InsufficientFunds));
    }

    #[test]
    fn unplayed_check_expires() {
        let d = db();
        let mut rt = EvaRuntime::new();
        rt.set_should_play(EvaMessage::InsufficientFunds);
        // Queued at frame 2, but speech is playing so it never plays.
        assert_eq!(rt.update(2, false, true, &d), None);
        // Expiration is 5000ms = 150 frames; past 152 it is pruned, so the message can queue again.
        assert_eq!(rt.update(300, false, false, &d), None);
        rt.set_should_play(EvaMessage::InsufficientFunds);
        assert_eq!(rt.update(301, false, false, &d), Some(EvaMessage::InsufficientFunds));
    }

    #[test]
    fn frames_between_checks_throttles() {
        let d = db();
        let mut rt = EvaRuntime::new();
        assert_eq!(rt.update(2, true, false, &d), Some(EvaMessage::LowPower));
        // Still in m_checks until playFrame + 3600.
        assert_eq!(rt.update(3, true, false, &d), None);
        // The check is pruned on the pass at frame 3603, so the message can re-queue at 3604
        // (`Eva::update` runs the check loop before `processPlayingMessages`).
        assert_eq!(rt.update(2 + 3601, true, false, &d), None);
        assert_eq!(rt.update(2 + 3602, true, false, &d), Some(EvaMessage::LowPower));
    }

    #[test]
    fn superweapon_message_classifies_relationship() {
        assert_eq!(superweapon_message_own("ready", SuperweaponKind::Nuke), EvaMessage::SuperweaponReady_Own_Nuke);
        assert_eq!(
            superweapon_message("launched", SuperweaponKind::ParticleCannon, Relationship::Enemies),
            EvaMessage::SuperweaponLaunched_Enemy_ParticleCannon
        );
        assert_eq!(
            superweapon_message("detected", SuperweaponKind::ScudStorm, Relationship::Neutral),
            EvaMessage::SuperweaponDetected_Ally_ScudStorm
        );
    }

    #[test]
    fn superweapon_kind_matches_enum_names() {
        assert_eq!(superweapon_kind("SPECIAL_PARTICLE_UPLINK_CANNON"), Some(SuperweaponKind::ParticleCannon));
        assert_eq!(superweapon_kind("SUPW_SPECIAL_NEUTRON_MISSILE"), Some(SuperweaponKind::Nuke));
        assert_eq!(superweapon_kind("SPECIAL_SCUD_STORM"), Some(SuperweaponKind::ScudStorm));
        assert_eq!(superweapon_kind("SPECIAL_BLACKLOTUS_CAPTURE_BUILDING"), None);
    }
}
