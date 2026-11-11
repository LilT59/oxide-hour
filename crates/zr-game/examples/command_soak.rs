//! Headless stress/soak harness: start a real skirmish (one human slot + AI slots) and, every
//! frame, issue a deterministic pseudo-random stream of **human-player** `Command`s drawn from
//! every `Command` variant. The harness's own RNG is a separate SplitMix64 stream, seeded, so runs
//! are reproducible. Each config runs twice; the two runs' CRCs must match every 300 frames, and a
//! panic is caught per run and reported with the frame and the last commands.
//!
//! The spec for what an invalid command does is the engine's dispatch: `GameLogicDispatch` and the
//! `ActionManager` legality checks normally *ignore* a message whose ids are dead, foreign or
//! nonexistent, or whose target/position is nonsense. A panic or a CRC divergence therefore means a
//! port bug, never an expected outcome.
//!
//! `cargo run --release -p zr-game --example command_soak [--frames N] [--seed N] [--replay]`
//! (needs a Zero Hour install, or `ZR_ZEROHOUR_DIR`).

use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use zr_assets::{Installs, Vfs};
use zr_game::replay::{Replay, ReplayHeader};
use zr_game::{Game, GameData, PlayerSetup};
use zr_sim::special_power::SpecialPowerTarget;
use zr_sim::template::kind_of;
use zr_sim::{Command, CombatDropTarget, Coord3D, GuardMode, GuardTarget, ObjectId, PlayerIndex};

/// Number of `Command` variants the generator can emit (possession build: no `Possess*`).
const VARIANTS: usize = 37;
/// The variant order of [`gen_command`], for the coverage report.
const VARIANT_NAMES: [&str; VARIANTS] = [
    "MoveTo", "Attack", "AttackGround", "ForceAttack", "Stop", "AttackMove", "Guard", "Sell", "Enter", "Exit", "Evacuate", "QueueUnit", "QueueUpgrade",
    "CancelUnit", "SetRallyPoint", "Construct", "CancelConstruct", "Repair", "ResumeConstruction", "Dock", "GetRepaired", "GetHealed", "DoSpecialPower",
    "PurchaseScience", "ForceMove", "AddWaypoint", "Scatter", "ToggleOvercharge", "SwitchWeapons", "CancelUpgrade", "CombatDrop", "DoWeapon",
    "DoWeaponAtObject", "DoWeaponAtLocation", "HackInternet", "SetMineClearingDetail", "DoSpecialPowerAtLocation",
];
/// CRC checkpoint interval.
const CRC_INTERVAL: u32 = 300;
/// Frames of commands kept for the panic report.
const LAST_KEEP: usize = 16;

/// SplitMix64: a tiny, fully deterministic, platform-independent PRNG that is *not* the sim RNG.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }
    fn chance(&mut self, pct: u32) -> bool {
        self.next_u64() % 100 < u64::from(pct)
    }
    fn f32_range(&mut self, lo: f32, hi: f32) -> f32 {
        let t = self.next_u64() as f64 / u64::MAX as f64;
        lo + (hi - lo) * t as f32
    }
}

/// A per-frame snapshot of the live object list, split into the selectors the generator uses.
#[derive(Default)]
struct Snap {
    any: Vec<ObjectId>,
    own: Vec<ObjectId>,
    own_mobile: Vec<ObjectId>,
    own_struct: Vec<ObjectId>,
    /// Own structures that are not a command center, so `Sell` cannot scuttle the base outright.
    own_non_cc: Vec<ObjectId>,
    own_dozer: Vec<ObjectId>,
    own_air: Vec<ObjectId>,
    /// Own units with `HackInternetAIUpdate`.
    own_hackers: Vec<ObjectId>,
    containers: Vec<ObjectId>,
    with_contained: Vec<ObjectId>,
    dock_targets: Vec<ObjectId>,
    repair_pads: Vec<ObjectId>,
    heal_pads: Vec<ObjectId>,
    enemies: Vec<ObjectId>,
}

fn snapshot(game: &Game, human: PlayerIndex) -> Snap {
    let mut s = Snap::default();
    for o in game.logic().objects() {
        if o.is_destroyed() {
            continue;
        }
        let id = o.id;
        s.any.push(id);
        let own = o.player == human;
        if own {
            s.own.push(id);
            if o.locomotor().is_some() {
                s.own_mobile.push(id);
            }
            if o.is_kind_of(kind_of::STRUCTURE) {
                s.own_struct.push(id);
                if !o.is_kind_of(kind_of::COMMANDCENTER) {
                    s.own_non_cc.push(id);
                }
            }
            if o.is_kind_of(kind_of::DOZER) {
                s.own_dozer.push(id);
            }
            if o.is_kind_of(kind_of::AIRCRAFT) {
                s.own_air.push(id);
            }
            if o.template.hack_internet.is_some() {
                s.own_hackers.push(id);
            }
        } else if o.player != 0 {
            s.enemies.push(id);
        }
        if o.contained.iter().any(|c| game.logic().find_object(*c).is_some()) {
            s.with_contained.push(id);
        } else if o.template.contain.is_some() {
            s.containers.push(id);
        }
        if o.template.dock.is_some() {
            s.dock_targets.push(id);
        }
        if o.is_kind_of(kind_of::REPAIR_PAD) || o.template.is_collide_kind_of(zr_sim::template::collide_kind::FS_AIRFIELD) {
            s.repair_pads.push(id);
        }
        if o.is_kind_of(kind_of::HEAL_PAD) {
            s.heal_pads.push(id);
        }
    }
    s
}

/// Real template / special-power / science / upgrade names from the install, so the harness can
/// issue both valid and invalid requests.
struct Names {
    templates: Vec<String>,
    build: Vec<String>,
    specials: Vec<String>,
    sciences: Vec<String>,
    upgrades: Vec<String>,
}

fn names(data: &GameData, human_faction: &str) -> Names {
    let t = data.templates();
    let templates: Vec<String> = t.things().iter().map(|th| th.name.clone()).collect();
    let mut build: Vec<String> = Vec::new();
    if let Some(pt) = data.player_template(human_faction) {
        if !pt.starting_building.is_empty() {
            build.push(pt.starting_building.clone());
        }
        build.extend(pt.starting_units.iter().cloned());
    }
    // A few common production templates so `QueueUnit` / `Construct` usually hit real data.
    for extra in [
        "AmericaVehicleDozer",
        "AmericaInfantryRanger",
        "AmericaTankCrusader",
        "AmericaVehicleHumvee",
        "ChinaVehicleDozer",
        "ChinaInfantryRedguard",
        "ChinaTankBattleMaster",
        "ChinaVehicleSupplyTruck",
        "GLAInfantryWorker",
        "GLAInfantryRebel",
        "GLAVehicleScorpion",
        "AmericaSupplyCenter",
        "ChinaSupplyCenter",
        "GLASupplyStash",
        "AmericaBarracks",
        "ChinaBarracks",
        "GLABarracks",
        "AmericaWarFactory",
        "ChinaWarFactory",
        "GLAArmsDealer",
    ] {
        if t.thing(extra).is_some() {
            build.push(extra.to_owned());
        }
    }
    build.sort();
    build.dedup();
    let specials = t.special_powers.iter().map(|p| p.name.clone()).collect();
    let sciences = t.sciences.iter().map(|s| s.name.clone()).collect();
    let mut upgrades = Vec::new();
    if let Some(db) = data.ini_db() {
        for b in db.of_kind("Upgrade") {
            if let Some(n) = b.name() {
                upgrades.push(n.to_owned());
            }
        }
    }
    upgrades.sort();
    upgrades.dedup();
    Names { templates, build, specials, sciences, upgrades }
}

impl Names {
    /// A template name: usually a real buildable one, sometimes any real template, sometimes garbage.
    fn template(&self, rng: &mut Rng) -> String {
        match rng.below(10) {
            0..=5 if !self.build.is_empty() => self.build[rng.below(self.build.len())].clone(),
            6..=8 if !self.templates.is_empty() => self.templates[rng.below(self.templates.len())].clone(),
            _ => format!("NotATemplate{}", rng.below(1000)),
        }
    }
    fn special(&self, rng: &mut Rng) -> String {
        if !self.specials.is_empty() && rng.chance(75) {
            self.specials[rng.below(self.specials.len())].clone()
        } else {
            format!("FakePower{}", rng.below(1000))
        }
    }
    fn science(&self, rng: &mut Rng) -> String {
        if !self.sciences.is_empty() && rng.chance(75) {
            self.sciences[rng.below(self.sciences.len())].clone()
        } else {
            format!("FakeScience{}", rng.below(1000))
        }
    }
    fn upgrade(&self, rng: &mut Rng) -> String {
        if !self.upgrades.is_empty() && rng.chance(75) {
            self.upgrades[rng.below(self.upgrades.len())].clone()
        } else {
            format!("FakeUpgrade{}", rng.below(1000))
        }
    }
}

/// One id from `list`, or a deliberately invalid id `invalid_pct`% of the time. An empty `list`
/// always yields an invalid id, so the invalid path is still exercised once the base is gone.
fn pick(rng: &mut Rng, list: &[ObjectId], invalid_pct: u32) -> ObjectId {
    if list.is_empty() || rng.chance(invalid_pct) {
        invalid_id(rng)
    } else {
        list[rng.below(list.len())]
    }
}

fn invalid_id(rng: &mut Rng) -> ObjectId {
    match rng.below(4) {
        0 => ObjectId::INVALID,
        1 => ObjectId(999_999),
        2 => ObjectId((rng.next_u64() as u32) | 0x8000_0000),
        _ => ObjectId(rng.below(6) as u32),
    }
}

/// A selection of 1..=4 ids from `list` (invalid ids if empty). Occasionally empty (the engine's
/// group helpers are no-ops on a zero-member group) or with a repeated id.
fn group(rng: &mut Rng, list: &[ObjectId]) -> Vec<ObjectId> {
    if rng.below(20) == 0 {
        return Vec::new();
    }
    let n = if list.is_empty() { 1 } else { 1 + rng.below(list.len().min(4)) };
    let mut v: Vec<ObjectId> = (0..n).map(|_| pick(rng, list, 20)).collect();
    if rng.chance(15) && !v.is_empty() {
        v.push(v[0]);
    }
    v
}

/// A weapon slot: one of the three real slots, or an out-of-range one (the dispatch ignores it).
fn weapon_slot(rng: &mut Rng) -> usize {
    if rng.chance(10) {
        3 + rng.below(5)
    } else {
        rng.below(3)
    }
}

/// `maxShotsToFire`: unlimited (negative), none, a few, or the engine's `NO_MAX_SHOTS_LIMIT`.
fn max_shots(rng: &mut Rng) -> i32 {
    match rng.below(5) {
        0 => -1,
        1 => i32::MIN,
        2 => 0,
        3 => 1 + rng.below(10) as i32,
        _ => i32::MAX,
    }
}

/// A position: in-map, off-map, on an edge, or a finite extreme. Never NaN.
fn position(rng: &mut Rng, world: (f32, f32)) -> Coord3D {
    let (w, h) = world;
    match rng.below(7) {
        0 => Coord3D::new(rng.f32_range(w * 0.1, w * 0.9), rng.f32_range(h * 0.1, h * 0.9), 0.0),
        1 => Coord3D::new(rng.f32_range(-w, 2.0 * w), rng.f32_range(-h, 2.0 * h), 0.0),
        2 => Coord3D::new(0.0, rng.f32_range(0.0, h), 0.0),
        3 => Coord3D::new(w, rng.f32_range(0.0, h), 0.0),
        4 => match rng.below(3) {
            0 => Coord3D::new(1.0e7, -1.0e7, 0.0),
            1 => Coord3D::new(f32::MAX, f32::MIN, 0.0),
            _ => Coord3D::new(f32::MAX / 2.0, f32::MAX / 2.0, f32::MIN / 2.0),
        },
        5 => Coord3D::new(rng.f32_range(-1.0e6, 1.0e6), rng.f32_range(-1.0e6, 1.0e6), 0.0),
        _ => Coord3D::new(rng.f32_range(0.0, w), rng.f32_range(0.0, h), rng.f32_range(-50.0, 200.0)),
    }
}

/// Build one command of `variant` from the snapshot.
fn gen_command(variant: usize, rng: &mut Rng, _game: &Game, human: PlayerIndex, snap: &Snap, names: &Names, world: (f32, f32)) -> Command {
    let _ = human;
    match variant {
        0 => Command::MoveTo { ids: group(rng, &snap.own_mobile), pos: position(rng, world) },
        1 => {
            let target = if !snap.enemies.is_empty() && rng.chance(80) { pick(rng, &snap.enemies, 10) } else { pick(rng, &snap.any, 25) };
            Command::Attack { ids: group(rng, &snap.own_mobile), target }
        }
        2 => Command::AttackGround { ids: group(rng, &snap.own_mobile), pos: position(rng, world) },
        3 => {
            // Force-attack normally hits allies too (that is the point of the command); bias to
            // enemies so the focused player is not ground down by its own orders.
            let target = if !snap.enemies.is_empty() && rng.chance(70) { pick(rng, &snap.enemies, 10) } else { pick(rng, &snap.any, 25) };
            Command::ForceAttack { ids: group(rng, &snap.own_mobile), target }
        }
        4 => Command::Stop { ids: group(rng, &snap.own) },
        5 => Command::AttackMove { ids: group(rng, &snap.own_mobile), pos: position(rng, world) },
        6 => {
            let target = match rng.below(3) {
                0 => GuardTarget::Point(position(rng, world)),
                _ => GuardTarget::Object(pick(rng, &snap.any, 25)),
            };
            let mode = match rng.below(3) {
                0 => GuardMode::Normal,
                1 => GuardMode::WithoutPursuit,
                _ => GuardMode::FlyingUnitsOnly,
            };
            Command::Guard { ids: group(rng, &snap.own_mobile), target, mode }
        }
        7 => Command::Sell { building: pick(rng, &snap.own_non_cc, 30) },
        8 => Command::Enter { ids: group(rng, &snap.own_mobile), container: pick(rng, &snap.any, 30) },
        9 => Command::Exit { container: pick(rng, &snap.with_contained, 40), ids: group(rng, &snap.own_mobile) },
        10 => Command::Evacuate { container: pick(rng, &snap.with_contained, 40) },
        11 => Command::QueueUnit { factory: pick(rng, &snap.own_struct, 25), template: names.template(rng) },
        12 => Command::QueueUpgrade { building: pick(rng, &snap.own_struct, 25), upgrade: names.upgrade(rng) },
        13 => Command::CancelUnit { factory: pick(rng, &snap.own_struct, 25), index: rng.below(5) },
        14 => Command::SetRallyPoint { factory: pick(rng, &snap.own_struct, 25), pos: position(rng, world) },
        15 => Command::Construct {
            dozer: pick(rng, &snap.own_dozer, 25),
            template: names.template(rng),
            pos: position(rng, world),
            angle: if rng.chance(10) { f32::MAX } else { rng.f32_range(-100.0, 100.0) },
        },
        16 => Command::CancelConstruct { dozer: pick(rng, &snap.own_dozer, 25) },
        17 => Command::Repair { dozer: pick(rng, &snap.own_dozer, 25), target: pick(rng, &snap.own_struct, 30) },
        18 => Command::ResumeConstruction { dozer: pick(rng, &snap.own_dozer, 25), target: pick(rng, &snap.any, 30) },
        19 => Command::Dock { ids: group(rng, &snap.own_mobile), target: pick(rng, &snap.dock_targets, 25) },
        20 => Command::GetRepaired { ids: group(rng, &snap.own_mobile), target: pick(rng, &snap.repair_pads, 25) },
        21 => Command::GetHealed { ids: group(rng, &snap.own_mobile), target: pick(rng, &snap.heal_pads, 25) },
        22 => {
            let target = match rng.below(4) {
                0 => SpecialPowerTarget::None,
                1 => SpecialPowerTarget::Position(position(rng, world)),
                _ => SpecialPowerTarget::Object(pick(rng, &snap.any, 25)),
            };
            Command::DoSpecialPower { source: pick(rng, &snap.own, 25), power: names.special(rng), target }
        }
        23 => Command::PurchaseScience { science: names.science(rng) },
        24 => Command::ForceMove { ids: group(rng, &snap.own_mobile), pos: position(rng, world) },
        25 => Command::AddWaypoint { ids: group(rng, &snap.own_mobile), pos: position(rng, world) },
        26 => Command::Scatter { ids: group(rng, &snap.own_mobile) },
        27 => Command::ToggleOvercharge { ids: group(rng, &snap.own) },
        28 => Command::SwitchWeapons { ids: group(rng, &snap.own_mobile), slot: rng.below(3) },
        29 => Command::CancelUpgrade { building: pick(rng, &snap.own_struct, 25), upgrade: names.upgrade(rng) },
        30 => {
            let target = if rng.chance(50) {
                CombatDropTarget::Position(position(rng, world))
            } else {
                CombatDropTarget::Object(pick(rng, &snap.any, 25))
            };
            Command::CombatDrop { ids: group(rng, &snap.own_air), target }
        }
        31 => Command::DoWeapon { ids: group(rng, &snap.own_mobile), slot: weapon_slot(rng), max_shots: max_shots(rng) },
        32 => {
            let target = if !snap.enemies.is_empty() && rng.chance(70) { pick(rng, &snap.enemies, 10) } else { pick(rng, &snap.any, 25) };
            Command::DoWeaponAtObject { ids: group(rng, &snap.own_mobile), slot: weapon_slot(rng), target, max_shots: max_shots(rng) }
        }
        33 => Command::DoWeaponAtLocation { ids: group(rng, &snap.own_mobile), slot: weapon_slot(rng), pos: position(rng, world), max_shots: max_shots(rng) },
        34 => {
            // Mostly hackers, so the hack state machine runs; some mobile units, which the dispatch
            // clears as `privateHackInternet` does.
            let ids = if rng.chance(70) { group(rng, &snap.own_hackers) } else { group(rng, &snap.own_mobile) };
            Command::HackInternet { ids }
        }
        35 => Command::SetMineClearingDetail { ids: group(rng, &snap.own_mobile) },
        36 => Command::DoSpecialPowerAtLocation {
            source: pick(rng, &snap.own, 25),
            power: names.special(rng),
            pos: position(rng, world),
            angle: match rng.below(4) {
                0 => None,
                1 => Some(rng.f32_range(-7.0, 7.0)),
                2 => Some(f32::MAX),
                _ => Some(rng.f32_range(-1.0e6, 1.0e6)),
            },
        },
        _ => unreachable!(),
    }
}

/// Shared per-run progress, readable from outside `catch_unwind` when a frame panics.
#[derive(Default)]
struct Progress {
    frame: AtomicU32,
    issued: AtomicU64,
    last: Mutex<VecDeque<String>>,
}

impl Progress {
    fn note(&self, player: PlayerIndex, cmd: &Command) {
        let mut last = self.last.lock().unwrap();
        if last.len() == LAST_KEEP {
            last.pop_front();
        }
        last.push_back(format!("f{} p{player} {cmd:?}", self.frame.load(Ordering::Relaxed)));
    }
    fn record(&self, frame: u32) {
        self.frame.store(frame, Ordering::Relaxed);
    }
    fn dump(&self) -> Vec<String> {
        self.last.lock().unwrap().iter().cloned().collect()
    }
}

#[derive(Default, Clone)]
struct Outcome {
    frames: u32,
    issued: u64,
    variants: Vec<u64>,
    crcs: Vec<(u32, u32)>,
    panic: Option<String>,
    panic_frame: Option<u32>,
    panic_last: Vec<String>,
    /// End-of-run diagnostics: human-owned objects, all live objects, human money.
    own: usize,
    total: usize,
    money: u32,
}

struct RunOutput {
    outcome: Outcome,
    replay: Option<Replay>,
}

/// A run's configuration.
#[derive(Clone)]
struct Config {
    label: String,
    map: String,
    human: String,
    ais: Vec<String>,
}

fn configs() -> Vec<Config> {
    let c = |label: &str, map: &str, human: &str, ais: &[&str]| Config {
        label: label.to_owned(),
        map: map.to_owned(),
        human: human.to_owned(),
        ais: ais.iter().map(|s| s.to_string()).collect(),
    };
    vec![
        c("US-Alpine", "Alpine Assault", "FactionAmerica", &["FactionChina"]),
        c("CN-Desert", "Tournament Desert", "FactionChina", &["FactionGLA"]),
        c("GLA-Alpine", "Alpine Assault", "FactionGLA", &["FactionAmerica"]),
        c("USGen-Alpine", "Alpine Assault", "FactionAmericaLaserGeneral", &["FactionChinaTankGeneral"]),
        c("GLAgen-Desert", "Tournament Desert", "FactionGLAToxinGeneral", &["FactionAmericaLaserGeneral"]),
        c("US-FFA4", "Tournament Tundra", "FactionAmerica", &["FactionChina", "FactionGLA", "FactionAmerica"]),
    ]
}

fn setups_for(cfg: &Config, seed: u32) -> Vec<PlayerSetup> {
    // The focused slot also runs the skirmish AI: a *passive* human slot is eliminated within a few
    // thousand frames, after which every command is a no-op against a dead player and the soak stops
    // exercising anything. Running the AI on the focused slot keeps a live, built-out base to command
    // for the whole run while the commands still arrive exactly as a human's would (`is_ai` only
    // gates the AI driving, never `processCommandList`).
    let mut v = vec![PlayerSetup::ai(&cfg.human, 0).with_cash(20000).with_difficulty(zr_game::AiDifficulty::Normal)];
    for (i, f) in cfg.ais.iter().enumerate() {
        v.push(PlayerSetup::ai(f, (i + 1) as u8).with_difficulty(zr_game::AiDifficulty::Normal));
    }
    let _ = seed;
    v
}

/// State threaded through one run so it can be recorded/replayed.
struct RunEnv<'a> {
    data: &'a Arc<GameData>,
    vfs: &'a Vfs,
    cfg: &'a Config,
    frames: u32,
    seed: u32,
    record: bool,
    passive: bool,
    progress: &'a Arc<Progress>,
}

fn run_inner(env: &RunEnv<'_>) -> RunOutput {
    let setups = setups_for(env.cfg, env.seed);
    let mut game = Game::new_skirmish_with(env.data.clone(), env.vfs, &env.cfg.map, &setups, env.seed).expect("start skirmish");
    let human = game.skirmish_player(0).map(|p| p.index).expect("focused slot");
    let names = names(env.data, &env.cfg.human);
    let world = game.logic().terrain().world_size();
    let mut rng = Rng::new(0x5EED_5EED_1234_5678 ^ u64::from(env.seed));
    if env.record {
        game.start_recording(
            ReplayHeader { map: env.cfg.map.clone(), seed: env.seed, starting_cash: 20000, superweapon_restriction: 0, slots: setups.clone() },
            CRC_INTERVAL,
        );
    }
    let trace = std::env::var_os("ZR_SOAK_TRACE").is_some();
    let mut out = Outcome { variants: vec![0; VARIANTS], ..Default::default() };
    // Round-robin over every variant so coverage is guaranteed; the arguments are still random.
    let mut vcursor = rng.below(VARIANTS);
    for frame in 0..env.frames {
        env.progress.record(frame);
        let snap = snapshot(&game, human);
        // A deterministic burst of 0..=4 commands per frame (~2.1 average).
        let n = if env.passive {
            0
        } else {
            match rng.below(10) {
                0 => 0,
                1..=2 => 1,
                3..=5 => 2,
                6..=8 => 3,
                _ => 4,
            }
        };
        for _ in 0..n {
            let variant = vcursor;
            vcursor = (vcursor + 1) % VARIANTS;
            let cmd = gen_command(variant, &mut rng, &game, human, &snap, &names, world);
            // Occasionally submit as a foreign / nonexistent player: `processCommandList` must ignore
            // a command that does not belong to the issuer, never index past the player table.
            let issuer = if rng.chance(3) { (40 + rng.below(200)) as PlayerIndex } else { human };
            env.progress.note(issuer, &cmd);
            env.progress.issued.fetch_add(1, Ordering::Relaxed);
            out.variants[variant] += 1;
            out.issued += 1;
            game.submit(issuer, cmd);
        }
        game.step();
        if trace && frame % 2000 == 0 {
            let own = game.logic().objects().filter(|o| o.player == human && !o.is_destroyed()).count();
            let total = game.logic().objects().filter(|o| !o.is_destroyed()).count();
            eprintln!(
                "[{}] f{frame} own {own} money {} total {total}",
                env.cfg.label,
                game.player_money(human).unwrap_or(0)
            );
        }
        if frame % CRC_INTERVAL == 0 {
            let crc = game.crc();
            if env.record {
                game.record_crc();
            }
            out.crcs.push((frame, crc));
        }
    }
    out.frames = env.frames;
    out.own = game.logic().objects().filter(|o| o.player == human && !o.is_destroyed()).count();
    out.total = game.logic().objects().filter(|o| !o.is_destroyed()).count();
    out.money = game.player_money(human).unwrap_or(0);
    RunOutput { outcome: out, replay: if env.record { game.take_recording() } else { None } }
}

/// Run with a watchdog thread: if the frame counter does not advance for `timeout`, print the
/// stuck frame and its last commands, then abort (a hang would otherwise just look like a slow run).
fn run_guarded(env: &RunEnv<'_>, timeout: std::time::Duration) -> RunOutput {
    use std::sync::atomic::AtomicBool;
    let done = Arc::new(AtomicBool::new(false));
    let progress = env.progress.clone();
    let done2 = done.clone();
    let handle = std::thread::spawn(move || {
        let mut last_frame = progress.frame.load(Ordering::Relaxed);
        let mut last_change = std::time::Instant::now();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(2));
            if done2.load(Ordering::Relaxed) {
                return;
            }
            let f = progress.frame.load(Ordering::Relaxed);
            if f != last_frame {
                last_frame = f;
                last_change = std::time::Instant::now();
            } else if last_change.elapsed() >= timeout {
                eprintln!("HANG: watchdog stuck at frame {f}; last commands:");
                for l in progress.dump() {
                    eprintln!("    last: {l}");
                }
                std::process::abort();
            }
        }
    });
    let out = run_once(env);
    done.store(true, Ordering::Relaxed);
    let _ = handle.join();
    out
}

fn run_once(env: &RunEnv<'_>) -> RunOutput {
    match catch_unwind(AssertUnwindSafe(|| run_inner(env))) {
        Ok(v) => v,
        Err(p) => {
            let msg = if let Some(s) = p.downcast_ref::<&str>() {
                (*s).to_owned()
            } else if let Some(s) = p.downcast_ref::<String>() {
                s.clone()
            } else {
                "non-string panic".to_owned()
            };
            RunOutput {
                outcome: Outcome {
                    panic: Some(msg),
                    panic_frame: Some(env.progress.frame.load(Ordering::Relaxed)),
                    panic_last: env.progress.dump(),
                    variants: vec![0; VARIANTS],
                    ..Default::default()
                },
                replay: None,
            }
        }
    }
}

/// Replay a recorded run and compare the CRCs.
fn playback(data: &Arc<GameData>, vfs: &Vfs, replay: &Replay) -> Result<Vec<(u32, u32)>, String> {
    let mut game = Game::new_skirmish_with(data.clone(), vfs, &replay.header.map, &replay.header.slots, replay.header.seed)
        .map_err(|e| format!("start playback: {e}"))?;
    let by_frame = replay.by_frame();
    let last = replay.crcs.last().map_or(0, |(f, _)| *f);
    let mut crcs = Vec::new();
    for frame in 0..=last {
        if let Some(events) = by_frame.get(&frame) {
            for e in events {
                game.submit(e.player, e.command.clone());
            }
        }
        game.step();
        if frame % replay.crc_interval == 0 {
            // Match `Game::record_crc`, which uses the sim frame (after the step).
            crcs.push((game.frame(), game.crc()));
        }
    }
    Ok(crcs)
}

fn read_arg(args: &[String], key: &str) -> Option<String> {
    args.iter().position(|a| a == key).and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let data = Arc::new(GameData::load(&vfs));
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--list") {
        for p in &data.player_templates {
            println!("{} side={}", p.name, p.side);
        }
        return;
    }
    let frames: u32 = read_arg(&args, "--frames").and_then(|s| s.parse().ok()).unwrap_or(20_000);
    let seed: u32 = read_arg(&args, "--seed").and_then(|s| s.parse().ok()).unwrap_or(1);
    let want_replay = args.iter().any(|a| a == "--replay");
    let passive = args.iter().any(|a| a == "--passive");

    let only: Option<String> = read_arg(&args, "--only");
    let mut table: Vec<String> = Vec::new();
    let mut any_panic = false;
    let mut coverage = vec![0u64; VARIANTS];
    for (ci, cfg) in configs().into_iter().enumerate() {
        if let Some(sel) = &only {
            if !cfg.label.contains(sel.as_str()) && !cfg.map.contains(sel.as_str()) {
                continue;
            }
        }
        let progress_a = Arc::new(Progress::default());
        let progress_b = Arc::new(Progress::default());
        let env_a = RunEnv { data: &data, vfs: &vfs, cfg: &cfg, frames, seed, record: false, passive, progress: &progress_a };
        let env_b = RunEnv { data: &data, vfs: &vfs, cfg: &cfg, frames, seed, record: false, passive, progress: &progress_b };
        let ra = run_guarded(&env_a, std::time::Duration::from_secs(90)).outcome;
        let rb = run_guarded(&env_b, std::time::Duration::from_secs(90)).outcome;

        let do_replay = want_replay && table.is_empty();
        let _ = ci;
        let mut replay_note = String::new();
        if do_replay {
            let progress_r = Arc::new(Progress::default());
            let env_r = RunEnv { data: &data, vfs: &vfs, cfg: &cfg, frames, seed, record: true, passive, progress: &progress_r };
            let rr = run_guarded(&env_r, std::time::Duration::from_secs(90));
            match (rr.replay, rr.outcome.panic) {
                (Some(rec), None) => match playback(&data, &vfs, &rec) {
                    Ok(pc) if pc == rec.crcs => replay_note = format!("replay {} events, {} CRCs match", rec.events.len(), pc.len()),
                    Ok(pc) => {
                        let first = rec
                            .crcs
                            .iter()
                            .zip(pc.iter())
                            .position(|(a, b)| a != b)
                            .map(|i| format!("first diff at #{i}: rec {:?} play {:?}", rec.crcs.get(i), pc.get(i)))
                            .unwrap_or_else(|| "length differs".to_owned());
                        replay_note = format!("replay DESYNC: recording {} CRCs, playback {} CRCs, {first}", rec.crcs.len(), pc.len());
                    }
                    Err(e) => replay_note = format!("replay error: {e}"),
                },
                (_, Some(p)) => replay_note = format!("replay run panicked: {p}"),
                (None, None) => replay_note = "replay not recorded".to_owned(),
            }
        }

        for (i, v) in ra.variants.iter().enumerate() {
            coverage[i] += *v;
        }
        for (i, v) in rb.variants.iter().enumerate() {
            coverage[i] += *v;
        }
        let panics: Vec<&Outcome> = [&ra, &rb].into_iter().filter(|o| o.panic.is_some()).collect();
        let crc_match = ra.crcs == rb.crcs && ra.panic.is_none() && rb.panic.is_none();
        if !panics.is_empty() {
            any_panic = true;
        }
        for o in &panics {
            let frame = o.panic_frame.unwrap_or(0);
            eprintln!("PANIC [{}] frame {frame}: {}", cfg.label, o.panic.as_deref().unwrap_or(""));
            for l in &o.panic_last {
                eprintln!("    last: {l}");
            }
        }
        let status = if !panics.is_empty() {
            "PANIC"
        } else if crc_match {
            "OK"
        } else {
            "DESYNC"
        };
        let line = format!(
            "  {:<12} {:>6} frames  {:>6} cmds  {:<6} crc {}/{}  {}",
            cfg.label,
            ra.frames.max(rb.frames),
            ra.issued,
            status,
            ra.crcs.len(),
            rb.crcs.len(),
            if replay_note.is_empty() { String::new() } else { replay_note }
        );
        println!("{line}");
        if args.iter().any(|a| a == "--diag") {
            println!("    diag: own {} total {} money {}", ra.own, ra.total, ra.money);
        }
        table.push(line);
    }

    println!("\n=== command_soak ({frames} frames/config, seed {seed}) ===");
    for line in &table {
        println!("{line}");
    }
    let uncovered: Vec<&str> = VARIANT_NAMES
        .iter()
        .enumerate()
        .filter(|(i, _)| coverage[*i] == 0)
        .map(|(_, n)| *n)
        .collect();
    let min = coverage.iter().copied().min().unwrap_or(0);
    println!("variant coverage: {} variants, min {min}, uncovered {:?}", VARIANTS, uncovered);
    if any_panic {
        println!("result: PANICS FOUND");
    } else {
        println!("result: all configs deterministic, no panics");
    }
}
