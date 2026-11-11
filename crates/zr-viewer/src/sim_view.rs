//! Live `zr-game` simulation in the viewer: fixed 30 Hz stepping, interpolated rendering of
//! logic objects, and the shared ray helpers. RTS input lives in `rts_input.rs`, possession in `possess.rs`.
//!
//! The client only reads sim state; input reaches the sim as [`Command`]s through [`Game::submit`].

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use zr_assets::Vfs;
use zr_game::{Game, GameData, PlayerKind, PlayerSetup};
use zr_ini::IniDb;
use zr_map::{MapFile, MapObject};
use zr_sim::{Command, Coord3D, ObjectId, PlayerIndex, LOGICFRAMES_PER_SECOND};

use crate::anim;
use crate::convert;
use crate::source::{self, Ground, ModelPart};
use crate::team_color::{self, TeamColors};

pub(crate) const STEP: f32 = 1.0 / LOGICFRAMES_PER_SECOND as f32;
/// Never simulate more than this many frames per rendered frame (spiral-of-death guard).
const MAX_STEPS_PER_FRAME: u32 = 6;

/// Options from the command line.
#[derive(Clone, Default)]
pub struct SimArgs {
    pub disabled: bool,
    /// Logic frames to run before the first rendered frame.
    pub prerun: u32,
    /// Order the local player's dozer to the map centre at start (and select it).
    pub auto_move: bool,
    /// Debug `--construct-demo`: the local dozer starts an American power plant beside it (construction
    /// screenshots; time the build with `--sim-frames`).
    pub construct_demo: bool,
    /// Possess the America dozer (or first unit) after the pre-run.
    #[cfg(feature = "possession")]
    pub possess: bool,
    /// Debug: spawn this template near the local start and select it (and possess it, with
    /// feature `possession`; screenshots).
    pub possess_unit: Option<String>,
    /// Debug: start possession in first person (screenshots).
    #[cfg(feature = "possession")]
    pub first_person: bool,
    /// Debug: damage the local player's objects to REALLYDAMAGED (smoke/fire screenshots).
    pub damage: bool,
    /// Debug: spawn a Crusader and a Battlemaster facing each other near the local start and
    /// order them to attack (for screenshots of firing / shells / damaged models).
    pub duel: bool,
    /// Debug: focus the camera on the first AI player's base (screenshots of AI construction).
    pub focus_ai: bool,
    /// Debug: reveal the AI base with a local scout that then drives away, so the last-seen enemy
    /// structures are drawn as fogged ghost objects (screenshots).
    pub ghost_view: bool,
    /// Debug: board Rangers into a Chinook and order a combat drop near the local base
    /// (rope/rappel screenshots). Time the DROP phase with `--sim-frames`.
    pub chinook_demo: bool,
    /// Debug: spawn the America particle uplink cannon and fire its superweapon at a point near the
    /// local base (orbital beam screenshot). Time the beam with `--sim-frames`.
    pub superweapon_demo: bool,
    /// Scripted held keys for the possessed unit, run after the pre-run (`W:60,D:30`).
    #[cfg(feature = "possession")]
    pub script: Vec<crate::possess::ScriptStep>,
}

/// Interpolation snapshot of one object.
#[derive(Clone, Copy)]
pub(crate) struct Snap {
    pub pos: [f32; 3],
    pub angle: f32,
}

/// Not `Send`/`Sync` (the sim holds `dyn` modules), so it lives in a non-send resource.
pub struct SimRes {
    pub(crate) game: Game,
    pub(crate) acc: f32,
    pub(crate) prev: BTreeMap<ObjectId, Snap>,
    pub(crate) cur: BTreeMap<ObjectId, Snap>,
    pub(crate) local: PlayerIndex,
    pub(crate) selected: Vec<ObjectId>,
    #[cfg(feature = "possession")]
    pub(crate) possess: crate::possess::PossessState,
}

/// `Object::isMassSelectable` test: `isKindOf(KINDOF_STRUCTURE)` on the live object.
pub(crate) fn is_structure(game: &Game, id: ObjectId) -> bool {
    game.logic().find_object(id).is_some_and(|o| o.template.is_kind_of(zr_sim::template::kind_of::STRUCTURE))
}

impl SimRes {
    /// The unit the local player possesses (always `None` without feature `possession`).
    pub(crate) fn possessed(&self) -> Option<ObjectId> {
        #[cfg(feature = "possession")]
        return self.possess.id;
        #[cfg(not(feature = "possession"))]
        None
    }
}

type Parts = Vec<(Handle<Mesh>, Handle<StandardMaterial>)>;

/// Models, loaded lazily per model name / (model, house colour).
#[derive(Resource)]
pub(crate) struct ModelCache {
    pub(crate) ini: IniDb,
    pub(crate) vfs: Vfs,
    colors: BTreeMap<PlayerIndex, [u8; 3]>,
    /// Source parts by template (fallback placeholders).
    raw: HashMap<String, Option<Vec<ModelPart>>>,
    /// Source parts by model file name (condition-state models).
    model_src: HashMap<String, Option<Vec<ModelPart>>>,
    parts: HashMap<(String, [u8; 3]), Parts>,
}

impl ModelCache {
    /// An empty cache over the install's INI / VFS (the shell map's live sim uses it too).
    pub(crate) fn new(ini: IniDb, vfs: Vfs, colors: BTreeMap<PlayerIndex, [u8; 3]>) -> Self {
        ModelCache { ini, vfs, colors, raw: HashMap::new(), model_src: HashMap::new(), parts: HashMap::new() }
    }

    /// The house colour of `owner`, neutral when it has none (`ThePlayerList` colour, `team_color`).
    pub(crate) fn color_of(&self, owner: PlayerIndex) -> [u8; 3] {
        *self.colors.get(&owner).unwrap_or(&team_color::NEUTRAL)
    }
}

#[derive(Component)]
pub(crate) struct SimObject(pub(crate) ObjectId);

/// The model file name currently drawn for a sim object (for condition-state model swaps), and the
/// inputs it was chosen from.
#[derive(Component)]
pub(crate) struct SimModel {
    model: String,
    key: ModelKey,
}

/// The inputs `desired_model` reads: the template, the `ModelConditionFlags` and the rappel state.
/// The model choice is a pure function of these, so `sim_sync` skips the (allocating) lookup while
/// they are the ones an entity was matched with.
#[derive(Clone, PartialEq)]
pub(crate) struct ModelKey {
    template: String,
    conditions: zr_sim::ModelConditionFlags,
    rappel: bool,
}

impl ModelKey {
    /// The key of sim object `id` as it is now, or `None` when it no longer exists.
    fn of(sim: &SimRes, id: ObjectId) -> Option<ModelKey> {
        let o = sim.game.object(id)?;
        Some(ModelKey { template: o.template.to_owned(), conditions: o.model_conditions, rappel: crate::chinook_view::rappelling(sim, id) })
    }

    /// Whether object `id` still has these inputs (no allocation).
    fn matches(&self, sim: &SimRes, id: ObjectId) -> bool {
        sim.game.object(id).is_some_and(|o| o.template == self.template && o.model_conditions == self.conditions)
            && crate::chinook_view::rappelling(sim, id) == self.rappel
    }
}

/// A rendered projectile, keyed by the sim projectile id. The sim owns the launch position
/// (`Weapon::positionProjectileForLaunch`), so the viewer draws it directly.
#[derive(Component)]
pub(crate) struct SimProjectile {
    id: u32,
}

/// The started game, before it is installed as a resource (the static pass needs it first).
pub struct SimInit {
    game: Game,
    selected: Vec<ObjectId>,
    #[cfg(feature = "possession")]
    possess: Option<crate::possess::PossessStart>,
    /// Debug `--duel`: the camera focus (midpoint of the two tanks).
    focus: Option<Vec3>,
    /// Debug `ZR_CONTEXT_DEMO`: the object whose context cursor is forced for a screenshot.
    forced_hover: Option<ObjectId>,
    /// `MultiplayerColor` table (from the shell) for chosen slot colours.
    color_table: Vec<[u8; 3]>,
}

impl SimInit {
    /// Start a skirmish from the setup screen's slots. `None` when disabled or the map cannot host
    /// it (the viewer then draws everything statically).
    pub fn new(vfs: &Vfs, map_name: &str, slots: &[PlayerSetup], color_table: &[[u8; 3]], seed: u32, args: SimArgs) -> Option<SimInit> {
        if args.disabled {
            return None;
        }
        let data = Arc::new(GameData::load(vfs));
        let setups: Vec<PlayerSetup> = if slots.is_empty() {
            vec![PlayerSetup::new("FactionAmerica", 0), PlayerSetup::ai("FactionChina", 1)]
        } else {
            slots.to_vec()
        };
        // The full skirmish path (`GameLogic::startNewGame`): loads the map, the per-player
        // qualified skirmish scripts and the victory players, exactly like `new_skirmish_with`.
        match Game::new_skirmish_with(data, vfs, map_name, &setups, seed) {
            Ok(game) => {
                let s = game.map_load_stats();
                let scripts = game.logic().scripts();
                info!(
                    "sim: {} map objects -> {} logic objects, {} props/trees stay static; scripts {} ({} lists)",
                    s.map_objects, s.created, s.props_or_trees_skipped, scripts.script_driven, scripts.lists.len()
                );
                Some(Self::start(game, color_table.to_vec(), args))
            }
            Err(e) => {
                warn!("sim disabled: {e:#}");
                None
            }
        }
    }

    fn start(mut game: Game, color_table: Vec<[u8; 3]>, args: SimArgs) -> SimInit {
        let local = game.skirmish_player(0).map_or(0, |p| p.index);
        let mut selected = Vec::new();
        let mut focus = None;
        let mut forced_hover = None;
        #[cfg(feature = "possession")]
        let mut possess_target = None;
        if let Some(template) = &args.possess_unit {
            // Debug: spawn the requested unit in the open near the local start and possess it.
            if let Some(start) = game.spawned().iter().find(|s| s.player == local).map(|s| s.pos) {
                let pos = Coord3D::new(start.x + 90.0, start.y + 90.0, 0.0);
                if let Some(id) = game.spawn_object(template, pos, 0.0, local) {
                    selected.push(id);
                    #[cfg(feature = "possession")]
                    {
                        possess_target = Some(id);
                    }
                    focus = Some(convert::to_bevy([pos.x, pos.y, pos.z]));
                }
            }
        } else if args.duel {
            if let Some((id, f)) = setup_duel(&mut game, local) {
                selected.push(id);
                focus = Some(f);
            }
        } else if args.focus_ai {
            let ai = game.players().iter().find(|p| p.is_ai).map(|p| p.index);
            if let Some(ai) = ai {
                if let Some(sp) = game.spawned().iter().find(|s| s.player == ai && s.role == "StartingBuilding") {
                    focus = Some(convert::to_bevy([sp.pos.x, sp.pos.y, sp.pos.z]));
                }
            }
        } else if args.ghost_view {
            // Debug: drop a local scout on the AI base, reveal the base, then drive it to the local
            // start. The reveal is undone as it leaves (`UnlookPersistDuration`), so the cells fall
            // back to fog and the enemy structures draw as last-seen ghost objects.
            let ai = game.players().iter().find(|p| p.is_ai).map(|p| p.index);
            if let Some(ai) = ai {
                if let Some(sp) = game.spawned().iter().find(|s| s.player == ai && s.role == "StartingBuilding") {
                    let base = sp.pos;
                    let scout = Coord3D::new(base.x, base.y - 150.0, 0.0);
                    if let Some(id) = game.spawn_object("AmericaVehicleHumvee", scout, 0.0, local) {
                        let dest = game
                            .spawned()
                            .iter()
                            .find(|s| s.player == local && s.role == "StartingBuilding")
                            .map_or(Coord3D::new(1363.0, 1966.0, 0.0), |s| s.pos);
                        game.submit(local, Command::MoveTo { ids: vec![id], pos: dest });
                        for _ in 0..260 {
                            game.step(); // scout leaves; base cells fall to fog
                        }
                        // Alpine's skirmish script fires the spy-satellite power, which drops a
                        // local `SpySatellitePing` over the enemy base. Its own `DeletionUpdate`
                        // destroys it after 13 s (retail), so just wait it out; once it is gone the
                        // shroud unlooks and the base fogs.
                        for _ in 0..300 {
                            let ping = game
                                .logic()
                                .objects()
                                .any(|o| o.player == local && o.template.name.contains("SpySatellite"));
                            if !ping {
                                break;
                            }
                            game.step();
                        }
                        // End while the ping's unlook-persist still keeps the base clear, so the
                        // viewer records it in `last_clear` before the cells fog; the ghost snapshot
                        // then appears as the viewer advances the sim.
                        for _ in 0..20 {
                            game.step();
                        }
                    }
                    focus = Some(convert::to_bevy([base.x, base.y, base.z]));
                }
            }
        } else if args.chinook_demo {
            // Debug: load Rangers into a Chinook and order a ground combat drop near the base, so a
            // screenshot can show the ropes and the rappelling infantry.
            if let Some(start) = game.spawned().iter().find(|s| s.player == local).map(|s| s.pos) {
                let from = Coord3D::new(start.x + 150.0, start.y + 60.0, 0.0);
                if let Some(ch) = game.spawn_unit(local, "AmericaVehicleChinook", from, 0.0) {
                    let mut riders = Vec::new();
                    for i in 0..4 {
                        let pos = Coord3D::new(from.x + 12.0 + i as f32 * 4.0, from.y, 0.0);
                        if let Some(r) = game.spawn_unit(local, "AmericaInfantryRanger", pos, 0.0) {
                            riders.push(r);
                        }
                    }
                    game.submit(local, Command::Enter { ids: riders, container: ch });
                    game.run_frames(150);
                    let target = Coord3D::new(start.x + 420.0, start.y + 120.0, 0.0);
                    if game.logic_mut().start_combat_drop(ch, target, None) {
                        focus = Some(convert::to_bevy([target.x, target.y, target.z]));
                        selected.push(ch);
                        info!("chinook demo: combat drop {} ordered to ({:.0}, {:.0})", ch.0, target.x, target.y);
                    }
                }
            }
        } else if args.superweapon_demo {
            // Debug: spawn the particle uplink cannon and fire its power (needs the science, which
            // the AI normally researches; grant all so the beam can be captured).
            let sciences: Vec<String> = game.data().templates().sciences.iter().map(|s| s.name.clone()).collect();
            for s in sciences {
                game.logic_mut().add_science(local, &s);
            }
            if let Some(start) = game.spawned().iter().find(|s| s.player == local).map(|s| s.pos) {
                let pos = Coord3D::new(start.x + 120.0, start.y - 160.0, 0.0);
                let target = Coord3D::new(pos.x + 260.0, pos.y + 40.0, 0.0);
                if let Some(id) = game.spawn_object("AmericaParticleCannonUplink", pos, 0.0, local) {
                    selected.push(id);
                    let power = game.logic().find_object(id).and_then(|o| o.template.particle_uplink.as_ref()).and_then(|p| p.special_power.clone());
                    if let Some(power) = power {
                        game.submit(local, Command::DoSpecialPower { source: id, power, target: zr_sim::special_power::SpecialPowerTarget::Position(target) });
                    }
                    // Frame the beam's actual target (the skirmish script may fire the power first).
                    game.run_frames(2);
                    let aim = game.logic().find_object(id).and_then(|o| o.superweapon.puc.as_deref()).map(|p| p.initial_target);
                    let frame_pos = aim.map_or([pos.x, pos.y, pos.z], |t| [t.x, t.y, t.z]);
                    focus = Some(convert::to_bevy(frame_pos));
                    info!("superweapon demo: particle uplink cannon {} firing at ({:.0}, {:.0})", id.0, frame_pos[0], frame_pos[1]);
                }
            }
        } else if args.construct_demo {
            // Debug `--construct-demo`: the dozer builds beside its start (the build runs in the pre-run).
            let dozer = game.spawned().iter().find(|s| s.player == local && s.template.contains("Dozer")).map(|s| s.id);
            let dozer = dozer.and_then(|id| game.object(id).map(|o| (id, o.position)));
            if let Some((id, p)) = dozer {
                // Orders are checked in turn; a refused site changes nothing and the first legal one makes
                // the dozer busy, so the rest are refused. Try a ring of sites around the dozer.
                let offsets = [(40.0f32, 0.0f32), (-40.0, 0.0), (0.0, 40.0), (0.0, -40.0), (50.0, 50.0), (-50.0, -50.0), (50.0, -50.0), (-50.0, 50.0)];
                for (dx, dy) in offsets {
                    let pos = Coord3D::new(p.x + dx, p.y + dy, 0.0);
                    game.submit(local, Command::Construct { dozer: id, template: "AmericaPowerPlant".to_owned(), pos, angle: 0.0 });
                }
                selected.push(id);
                focus = Some(convert::to_bevy([p.x + 40.0, p.y, p.z]));
                info!("construct demo: dozer {} asked to build AmericaPowerPlant beside ({:.0}, {:.0})", id.0, p.x, p.y);
            }
        } else if args.auto_move {
            let dozer = game.spawned().iter().find(|s| s.player == local && s.template.contains("Dozer")).map(|s| s.id);
            if let Some(id) = dozer {
                let (w, h) = game.logic().terrain().world_size();
                game.submit(local, Command::MoveTo { ids: vec![id], pos: Coord3D::new(w / 2.0, h / 2.0, 0.0) });
                selected.push(id);
            }
        } else if slope_demo_case() {
            if let Some((id, f)) = setup_slope_demo(&mut game, local) {
                selected.push(id);
                focus = Some(f);
            }
        } else if let Some(case) = context_demo_case() {
            // Debug `ZR_CONTEXT_DEMO`: spawn/damage the screenshot subject (build-time, like the
            // other debug flags), then order the real context command as a `Command`.
            if let Some((sel, target)) = crate::context_cmd::demo_setup(&mut game, local, &case) {
                crate::context_cmd::order_target(&mut game, local, &sel, target);
                focus = game.object(target).map(|o| convert::to_bevy([o.position.x, o.position.y, o.position.z]));
                info!("context-demo {case}: {} -> {target:?}", sel.first().map(|id| id.0).unwrap_or(0));
                selected = sel;
                forced_hover = Some(target);
            } else {
                warn!("context-demo: unknown case {case:?} (expected repair|enter|attack)");
            }
        }
        for _ in 0..args.prerun {
            game.step();
        }
        if args.damage {
            // Debug: knock every local object down to REALLYDAMAGED so its damaged smoke/fire plays.
            let ids: Vec<ObjectId> = game.spawned().iter().filter(|s| s.player == local).map(|s| s.id).collect();
            for id in ids {
                if let Some(o) = game.object(id) {
                    if o.health > 1.0 {
                        game.damage_object(id, o.health * 0.93);
                    }
                }
            }
            game.step();
        }
        #[cfg(feature = "possession")]
        let possess = if args.possess { crate::possess::auto_possess(&mut game, local, &args.script, possess_target, args.first_person) } else { None };
        #[cfg(feature = "possession")]
        if let Some(p) = &possess {
            selected = vec![p.id];
        }
        if let Some(o) = selected.first().and_then(|&id| game.object(id)) {
            info!("sim: dozer {} at ({:.1}, {:.1}) at frame {}", o.id.0, o.position.x, o.position.y, game.frame());
        }
        SimInit {
            game,
            selected,
            #[cfg(feature = "possession")]
            possess,
            focus,
            forced_hover,
            color_table,
        }
    }

    /// Whether a map entry is drawn from the simulation (skip it in the static pass).
    pub fn is_logic(&self, o: &MapObject) -> bool {
        self.game.is_logic_map_object(o)
    }

    /// The started game, so the static pass can size the shroud overlay before it is installed.
    pub(crate) fn game(&self) -> &Game {
        &self.game
    }

    /// Bevy-space focus for the camera: the local player's starting building (or the duel midpoint).
    pub fn focus(&self) -> Option<Vec3> {
        if let Some(f) = self.focus {
            return Some(f);
        }
        if let Some(o) = self.selected.first().and_then(|&id| self.game.object(id)) {
            return Some(convert::to_bevy([o.position.x, o.position.y, o.position.z]));
        }
        let p = self.game.skirmish_player(0)?.index;
        let s = self.game.spawned().iter().find(|s| s.player == p)?;
        Some(convert::to_bevy([s.pos.x, s.pos.y, s.pos.z]))
    }

    /// Install the resources the systems use.
    pub fn install(self, commands: &mut Commands, vfs: Vfs, ini: IniDb, map: &MapFile, meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>) -> SimRes {
        let local = self.game.skirmish_player(0).map_or(0, |p| p.index);
        let selected = self.selected;
        // `ZR_CONTEXT_DEMO`: keep the forced hover target for the cursor label.
        commands.insert_resource(crate::context_cmd::ForcedHover(self.forced_hover));
        #[cfg(feature = "possession")]
        let mut possess = crate::possess::PossessState::default();
        #[cfg(feature = "possession")]
        if let Some(p) = self.possess {
            possess.begin(p.id, p.yaw, p.frame);
            possess.first_person = p.first_person;
            if p.first_person {
                // The third-person default pitch looks ~19 degrees down; first person should look
                // roughly level so the screenshot / initial view shows the horizon.
                possess.pitch = 0.05;
            }
        }
        let cur = snapshot(&self.game);
        // Player colours: a chosen `MultiplayerColor` wins, else the faction's colour; map sides
        // keep the map's `playerColor`.
        let color_table = self.color_table;
        let mut colors = BTreeMap::new();
        for p in self.game.players() {
            let c = match p.kind {
                PlayerKind::Skirmish if p.color >= 0 => color_table.get(p.color as usize).copied().unwrap_or_else(|| TeamColors::faction_color(&ini, &p.faction)),
                PlayerKind::Skirmish => TeamColors::faction_color(&ini, &p.faction),
                PlayerKind::Neutral => team_color::NEUTRAL,
                _ => TeamColors::named_player(map, &ini, &p.name),
            };
            colors.insert(p.index, c);
        }
        commands.insert_resource(ModelCache { ini, vfs, colors, raw: HashMap::new(), model_src: HashMap::new(), parts: HashMap::new() });
        let _ = (meshes, materials);
        SimRes {
            game: self.game,
            acc: 0.0,
            prev: cur.clone(),
            cur,
            local,
            selected,
            #[cfg(feature = "possession")]
            possess,
        }
    }
}

fn snapshot(game: &Game) -> BTreeMap<ObjectId, Snap> {
    game.objects().map(|o| (o.id, Snap { pos: [o.position.x, o.position.y, o.position.z], angle: o.angle })).collect()
}

/// `ZR_CONTEXT_DEMO=<repair|enter|attack>` (see [`crate::context_cmd::demo_setup`]).
fn context_demo_case() -> Option<String> {
    std::env::var("ZR_CONTEXT_DEMO").ok().filter(|s| !s.is_empty())
}

/// `ZR_SLOPE_DEMO=1` (see [`setup_slope_demo`]).
fn slope_demo_case() -> bool {
    std::env::var("ZR_SLOPE_DEMO").is_ok_and(|s| !s.is_empty() && s != "0")
}

/// Debug `ZR_SLOPE_DEMO`: a Crusader facing uphill and a Humvee across the slope, standing on the
/// steepest 7-11 degree ground within 300 units of the local start, so `Drawable::calcPhysicsXform*`'s
/// terrain pitch and roll and the wheel suspension show. Returns the Crusader and the camera focus.
fn setup_slope_demo(game: &mut Game, local: PlayerIndex) -> Option<(ObjectId, Vec3)> {
    let start = game.spawned().iter().find(|s| s.player == local).map(|s| s.pos)?;
    let terrain = game.logic().terrain();
    // The slope gradient in world units per world unit, from the same samples `getGroundHeight` uses.
    let gradient = |x: f32, y: f32| {
        let d = crate::source::MAP_XY_FACTOR;
        ((terrain.ground_height(x + d, y) - terrain.ground_height(x - d, y)) / (2.0 * d), (terrain.ground_height(x, y + d) - terrain.ground_height(x, y - d)) / (2.0 * d))
    };
    let mut best = (0.0f32, start.x, start.y);
    let mut y = start.y - 300.0;
    while y <= start.y + 300.0 {
        let mut x = start.x - 300.0;
        while x <= start.x + 300.0 {
            let (gx, gy) = gradient(x, y);
            let m = gx * gx + gy * gy;
            // A gentle, drivable grass slope (7..11 degrees), so the demo does not land on a cliff.
            if m > best.0 && m > 0.12 * 0.12 && m < 0.20 * 0.20 {
                best = (m, x, y);
            }
            x += 10.0;
        }
        y += 10.0;
    }
    let (slope_sqr, x, y) = best;
    let (gx, gy) = gradient(x, y);
    let uphill = gy.atan2(gx);
    let z = terrain.ground_height(x, y);
    let a = Coord3D::new(x, y, z);
    let b = Coord3D::new(x + 16.0, y + 16.0, terrain.ground_height(x + 16.0, y + 16.0));
    let tank = game.spawn_object("AmericaTankCrusader", a, uphill, local)?;
    let humvee = game.spawn_object("AmericaVehicleHumvee", b, uphill + std::f32::consts::FRAC_PI_2, local)?;
    info!(
        "slope demo: {:.1} deg slope; Crusader {} on ({:.0},{:.0}) facing {:.1} deg uphill, Humvee {} across it; camera --camera {:.0},{:.0},80",
        slope_sqr.sqrt().atan().to_degrees(),
        tank.0,
        x,
        y,
        uphill.to_degrees(),
        humvee.0,
        x,
        y
    );
    Some((tank, convert::to_bevy([x, y, z])))
}

/// Debug `--duel`: a Crusader and a Battlemaster facing each other in the open, ordered to
/// attack. Returns the Crusader id and the camera focus (their midpoint).
fn setup_duel(game: &mut Game, local: PlayerIndex) -> Option<(ObjectId, Vec3)> {
    let china = game.skirmish_player(1)?.index;
    game.set_enemies(local, china);
    // A clear spot in the middle of the map, away from the starting base.
    let (w, h) = game.logic().terrain().world_size();
    let (cx, cy) = (w * 0.45, h * 0.33);
    let (ax, ay) = (cx - 18.0, cy);
    let (bx, by) = (cx + 18.0, cy);
    let terrain = game.logic().terrain();
    let (ga, gb, gc) = (terrain.ground_height(ax, ay), terrain.ground_height(bx, by), terrain.ground_height(cx, cy));
    let a = Coord3D::new(ax, ay, ga);
    let b = Coord3D::new(bx, by, gb);
    // `ZR_DUEL_A` / `ZR_DUEL_B` swap in other templates (e.g. a missile unit for exhaust screenshots).
    let name_a = std::env::var("ZR_DUEL_A").unwrap_or_else(|_| "AmericaTankCrusader".into());
    let name_b = std::env::var("ZR_DUEL_B").unwrap_or_else(|_| "ChinaTankBattleMaster".into());
    let crusader = game.spawn_object(&name_a, a, 0.0, local)?;
    let battlemaster = game.spawn_object(&name_b, b, std::f32::consts::PI, china)?;
    game.submit(local, Command::Attack { ids: vec![crusader], target: battlemaster });
    game.submit(china, Command::Attack { ids: vec![battlemaster], target: crusader });
    info!("duel: Crusader {} vs Battlemaster {} at ({:.0},{:.0})", crusader.0, battlemaster.0, cx, cy);
    Some((crusader, convert::to_bevy([cx, cy, gc])))
}

/// Hand-over slot from the setup system to [`adopt_sim`].
#[derive(Default)]
pub struct SimSlot(pub Option<SimRes>);

/// Exclusive system: move the started sim into a non-send resource.
pub(crate) fn adopt_sim(world: &mut World) {
    let Some(sim) = world.non_send_mut::<SimSlot>().0.take() else { return };
    world.insert_non_send(sim);
    world.insert_resource(SimActive);
}

/// Send-able marker that the non-send [`SimRes`] exists. Run conditions may be evaluated off the
/// main thread, so they must not touch `NonSend` data themselves.
#[derive(Resource)]
pub(crate) struct SimActive;

pub fn add_systems(app: &mut App) {
    app.insert_non_send(SimSlot::default());
    // The staged build (`build`) sets `SimSlot` while on the load screen; adopting it here hands
    // the sim to the live systems.
    app.add_systems(OnEnter(crate::shell::ShellState::InGame), adopt_sim);
    app.add_systems(Update, (sim_step, sim_sync, projectile_sync).chain().run_if(resource_exists::<SimActive>));
}

/// A decoded replay being played back (`--replay`): events to submit per frame and the CRCs to
/// check after stepping (`Recorder` playback).
#[derive(Resource)]
pub struct ReplayPlayback {
    by_frame: BTreeMap<u32, Vec<(u8, Command)>>,
    crcs: BTreeMap<u32, u32>,
    interval: u32,
    /// Frames whose CRC differed from the recording (0 = no desync).
    pub desyncs: u32,
    /// Frames checked.
    pub checked: u32,
}

impl ReplayPlayback {
    pub fn from_replay(r: &zr_game::replay::Replay) -> Self {
        let mut by_frame: BTreeMap<u32, Vec<(u8, Command)>> = BTreeMap::new();
        for e in &r.events {
            by_frame.entry(e.frame).or_default().push((e.player, e.command.clone()));
        }
        ReplayPlayback { by_frame, crcs: r.crcs.iter().copied().collect(), interval: r.crc_interval.max(1), desyncs: 0, checked: 0 }
    }

    /// Checks the CRC checkpoint for the sim's current frame (`Recorder` playback).
    pub fn check(&mut self, sim: &SimRes) {
        let frame = sim.game.frame();
        if frame % self.interval == 0 {
            if let Some(expected) = self.crcs.get(&frame).copied() {
                let got = sim.game.crc();
                self.checked += 1;
                if got != expected {
                    self.desyncs += 1;
                    warn!("replay desync at frame {frame}: {got:08x} != {expected:08x}");
                }
            }
        }
    }
}

/// Fixed 30 Hz stepping from an accumulator; wall-clock time never enters the sim itself.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sim_step(
    time: Res<Time>,
    mut sim: NonSendMut<SimRes>,
    mut fx_queue: ResMut<crate::fx::FxQueue>,
    paused: Res<crate::Paused>,
    mut replay: Option<ResMut<ReplayPlayback>>,
    settings: Option<Res<crate::options::GameSettings>>,
) {
    if paused.0 {
        return;
    }
    let sim = &mut *sim;
    sim.acc += time.delta_secs().min(0.25);
    let mut steps = 0;
    while sim.acc >= STEP && steps < MAX_STEPS_PER_FRAME {
        sim.acc -= STEP;
        // Replay playback: submit this frame's recorded commands before stepping.
        if let Some(pb) = replay.as_deref_mut() {
            if let Some(events) = pb.by_frame.get(&sim.game.frame()) {
                for (player, command) in events {
                    sim.game.submit(*player, command.clone());
                }
            }
        }
        // `Player::update`'s client half (`Player.cpp:697-718`): once a second, when `Retaliation` differs
        // from the local player's logical mode, post `MSG_ENABLE_RETALIATION_MODE` for it.
        if let Some(wanted) = settings.as_deref().map(|s| s.retaliation) {
            let logical = sim.game.logic().player(sim.local).is_some_and(|p| p.logical_retaliation_mode_enabled);
            if retaliation_post_due(sim.game.frame(), wanted, logical) {
                sim.game.submit(sim.local, Command::EnableRetaliationMode { enabled: wanted });
            }
        }
        // Possession input goes in before the step, one message per logic frame.
        #[cfg(feature = "possession")]
        if let Some(cmd) = sim.possess.frame_command() {
            let local = sim.local;
            sim.game.submit(local, cmd);
        }
        let t = crate::perf::start();
        sim.game.step();
        crate::perf::record_step(t);
        // FX events are cleared at the start of each logic frame, so drain them after every
        // step (a render frame may run several logic steps).
        for e in sim.game.logic().fx_events() {
            // The shot direction (turret, else body heading) for tracer / ray nuggets.
            let dir = sim.game.logic().find_object(e.object).map_or([0.0; 3], |o| {
                let a = if o.template.combat.turret_turn_rate.is_some() { o.combat.turret_angle } else { o.angle };
                [a.cos(), a.sin(), 0.0]
            });
            let aim = crate::fx::Aim { target: e.target.map(|t| [t.x, t.y, t.z]), weapon_speed: e.weapon_speed };
            fx_queue.0.push((e.object, e.name.clone(), [e.pos.x, e.pos.y, e.pos.z], dir, aim));
        }
        sim.prev = std::mem::take(&mut sim.cur);
        let t = crate::perf::start();
        sim.cur = snapshot(&sim.game);
        crate::perf::record_snapshot(t);
        steps += 1;
    }
    if steps == MAX_STEPS_PER_FRAME {
        sim.acc = 0.0;
    }
}

/// The client half of `Player::update`'s retaliation check (`Player.cpp:704-714`): the option is compared
/// against the player's logical mode once a second (`now % LOGICFRAMES_PER_SECOND == 0`).
fn retaliation_post_due(frame: u32, wanted: bool, logical: bool) -> bool {
    frame % LOGICFRAMES_PER_SECOND == 0 && wanted != logical
}

fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let mut d = (b - a) % TAU;
    if d > PI {
        d -= TAU;
    } else if d < -PI {
        d += TAU;
    }
    a + d * t
}

pub(crate) fn interpolated(sim: &SimRes, id: ObjectId, alpha: f32) -> Option<([f32; 3], f32)> {
    let c = sim.cur.get(&id)?;
    let p = sim.prev.get(&id).unwrap_or(c);
    let l = |a: f32, b: f32| a + (b - a) * alpha;
    Some(([l(p.pos[0], c.pos[0]), l(p.pos[1], c.pos[1]), l(p.pos[2], c.pos[2])], lerp_angle(p.angle, c.angle, alpha)))
}

/// The model file a sim object's current condition state selects (falls back to the template's
/// default model). Used to detect model swaps (damaged / rubble / dying).
fn desired_model(sim: &SimRes, anim_cache: &mut anim::AnimCache, ini: &IniDb, id: ObjectId) -> Option<String> {
    let o = sim.game.object(id)?;
    let flags = anim::view_conditions(sim, id, o.model_conditions);
    // No model: key the rig by the template name so it is stable frame to frame.
    Some(main_model(anim_cache, ini, o.template, &flags).unwrap_or_else(|| o.template.to_owned()))
}

/// The model the main draw module shows for `template` under `flags`: `Some("")` when its condition
/// state has no model (`Model = NONE`, which the engine draws as nothing, e.g. a structure awaiting its
/// builder), the state's model, or `None` when the template has no state table and no model.
pub(crate) fn main_model(anim_cache: &mut anim::AnimCache, ini: &IniDb, template: &str, flags: &zr_w3d::ConditionFlags) -> Option<String> {
    match anim_cache.states(ini, template) {
        Some(states) => match states.find_best_state(flags) {
            Some(st) => Some(st.model.clone().unwrap_or_default()),
            None => zr_w3d::model_for_template(ini, template),
        },
        None => zr_w3d::model_for_template(ini, template),
    }
}

/// Spawn / despawn / move entities for logic objects; respawns when the condition state changes
/// the model. Keeps the selection ring on the selection.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sim_sync(
    mut commands: Commands,
    sim: NonSend<SimRes>,
    mut cache: ResMut<ModelCache>,
    mut anim_cache: ResMut<anim::AnimCache>,
    mut module_cache: ResMut<crate::weapon_modules::WeaponModuleCache>,
    mut inv_bind: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut objs: Query<(Entity, &SimObject, &mut SimModel, &mut Transform, Option<&crate::shroud::GhostSnapshot>)>,
) {
    let alpha = (sim.acc / STEP).clamp(0.0, 1.0);
    let mut have = HashSet::new();
    for (e, so, mut model, mut t, ghost) in &mut objs {
        // A last-seen ghost is frozen at its snapshot state (`W3DGhostObject::snapShot` removes
        // the live render object and shows a static copy); keep it from being respawned even when
        // the underlying object has since died.
        if ghost.is_some() {
            have.insert(so.0);
            continue;
        }
        match interpolated(&sim, so.0, alpha) {
            Some((pos, ang)) => {
                // `W3DModelDraw::adjustTransformMtx`: the construction height offset (zero otherwise).
                let lift = crate::construction_view::height_offset(&sim, &mut anim_cache, &cache.ini, so.0);
                *t = convert::object_transform([pos[0], pos[1], pos[2] + lift], ang);
                // The model only changes when its inputs do (`ModelKey`), so skip the lookup otherwise.
                if model.key.matches(&sim, so.0) {
                    have.insert(so.0);
                    continue;
                }
                let desired = desired_model(&sim, &mut anim_cache, &cache.ini, so.0);
                if desired.as_deref() != Some(model.model.as_str()) {
                    info!("model swap object {} {} -> {:?}", so.0.0, model.model, desired);
                    commands.entity(e).despawn(); // model changed -> respawn below
                } else {
                    if let Some(key) = ModelKey::of(&sim, so.0) {
                        model.key = key;
                    }
                    have.insert(so.0);
                }
            }
            None => commands.entity(e).despawn(),
        }
    }
    let ModelCache { ini, vfs, colors, raw, model_src, parts } = &mut *cache;
    let no_parts: Vec<source::ModelPart> = Vec::new();
    for o in sim.game.objects() {
        // `Drawable::setDrawableHidden`: a train car waiting in the wings is not drawn (RailroadGuideAIUpdate.cpp:835).
        if have.contains(&o.id) || o.is_drawable_hidden() {
            continue;
        }
        let flags = anim::view_conditions(&sim, o.id, o.model_conditions);
        let model = main_model(&mut anim_cache, ini, o.template, &flags);
        // Source parts: the state's model when it resolves, none for `Model = NONE`, else the template fallback.
        let src = match model.as_deref() {
            Some("") => Some(&no_parts),
            Some(m) => model_src.entry(m.to_owned()).or_insert_with(|| source::model_by_name(vfs, m)).as_ref(),
            None => raw.entry(o.template.to_owned()).or_insert_with(|| source::object_model(ini, vfs, o.template)).as_ref(),
        };
        let Some(src) = src else { continue };
        let asset = anim_cache.asset(ini, vfs, &mut inv_bind, o.template);
        let pivots = asset.as_ref().map(|a| a.hierarchy.pivot_count());
        let key_model = model.unwrap_or_else(|| o.template.to_owned());
        let house = src.iter().any(team_color::uses_house_color);
        // `W3DModelDrawModuleData::m_okToChangeModelColor` (`replaceIndicatorColor`, W3DModelDraw.cpp:3206-3226): only
        // a module that opts in takes the player colour; the others keep their own house-colour material.
        let rgb = if house && module_cache.first_ok_to_change_model_color(ini, vfs, o.template) {
            *colors.get(&o.owner).unwrap_or(&team_color::NEUTRAL)
        } else {
            team_color::NEUTRAL
        };
        let ps = parts.entry((key_model.clone(), rgb)).or_insert_with(|| {
            src.iter()
                .map(|p| {
                    let tinted;
                    let p = if house && team_color::uses_house_color(p) {
                        tinted = team_color::tint(p, rgb);
                        &tinted
                    } else {
                        p
                    };
                    let (m, mat) = convert::model_part(p, &mut images, pivots);
                    (meshes.add(m), materials.add(mat))
                })
                .collect()
        });
        let (pos, ang) = interpolated(&sim, o.id, alpha).unwrap_or(([o.position.x, o.position.y, o.position.z], o.angle));
        let lift = crate::construction_view::height_offset(&sim, &mut anim_cache, ini, o.id);
        let pos = [pos[0], pos[1], pos[2] + lift];
        if std::env::var_os("ZR_LOG_OBJECTS").is_some() {
            info!("spawn object {} {} model {} at {:.0},{:.0},{:.0} parts {}", o.id.0, o.template, key_model, pos[0], pos[1], pos[2], src.len());
        }
        let root = commands
            .spawn((
                SimObject(o.id),
                SimModel {
                    model: key_model,
                    key: ModelKey { template: o.template.to_owned(), conditions: o.model_conditions, rappel: crate::chinook_view::rappelling(&sim, o.id) },
                },
                convert::object_transform(pos, ang),
                Visibility::default(),
            ))
            .id();
        anim::populate(&mut commands, root, src.as_slice(), ps.as_slice(), asset.as_ref(), &flags, &mut anim_cache, vfs);
    }
}

fn lerp_coord(a: Coord3D, b: Coord3D, t: f32) -> Coord3D {
    Coord3D::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, a.z + (b.z - a.z) * t)
}

/// Renders the sim's live projectiles with their template's model, oriented along velocity. The
/// sim already launches them from the firing barrel's `WeaponLaunchBone`, so no client offset is
/// applied (`Weapon::positionProjectileForLaunch`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn projectile_sync(
    mut commands: Commands,
    sim: NonSend<SimRes>,
    mut cache: ResMut<ModelCache>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut existing: Query<(Entity, &SimProjectile, &mut Transform)>,
) {
    let alpha = (sim.acc / STEP).clamp(0.0, 1.0);
    let snapshots = sim.game.projectiles();
    let live: HashSet<u32> = snapshots.iter().map(|s| s.id).collect();
    let mut have = HashSet::new();
    for (e, sp, mut t) in &mut existing {
        if !live.contains(&sp.id) {
            commands.entity(e).despawn();
            continue;
        }
        if let Some(s) = snapshots.iter().find(|s| s.id == sp.id) {
            let p = lerp_coord(s.prev_pos, s.pos, alpha);
            let yaw = f32::atan2(s.velocity.y, s.velocity.x);
            *t = convert::object_transform([p.x, p.y, p.z], yaw);
            have.insert(sp.id);
        }
    }
    let ModelCache { ini, vfs, raw, parts, .. } = &mut *cache;
    for s in &snapshots {
        if have.contains(&s.id) {
            continue;
        }
        let Some(src) = raw
            .entry(s.template_name.clone())
            .or_insert_with(|| source::object_model(ini, vfs, &s.template_name))
            .clone()
        else {
            continue;
        };
        let ps = parts.entry((s.template_name.clone(), team_color::NEUTRAL)).or_insert_with(|| {
            src.iter()
                .map(|p| {
                    let (m, mat) = convert::model_part(p, &mut images, None);
                    (meshes.add(m), materials.add(mat))
                })
                .collect()
        });
        let p = lerp_coord(s.prev_pos, s.pos, alpha);
        let yaw = f32::atan2(s.velocity.y, s.velocity.x);
        let transform = convert::object_transform([p.x, p.y, p.z], yaw);
        let root = commands.spawn((SimProjectile { id: s.id }, transform, Visibility::default())).id();
        commands.entity(root).with_children(|c| {
            for (m, mat) in ps.iter() {
                c.spawn((Mesh3d(m.clone()), MeshMaterial3d(mat.clone()), Transform::IDENTITY));
            }
        });
    }
}

/// Distance along a ray to a sphere, if hit.
pub(crate) fn ray_sphere(origin: Vec3, dir: Vec3, c: Vec3, r: f32) -> Option<f32> {
    let oc = origin - c;
    let b = oc.dot(dir);
    let disc = b * b - (oc.dot(oc) - r * r);
    if disc < 0.0 {
        return None;
    }
    let t = -b - disc.sqrt();
    (t >= 0.0).then_some(t)
}

/// March the ray down onto the terrain (Generals-space ground height), then bisect.
pub(crate) fn ray_ground(origin: Vec3, dir: Vec3, ground: &Ground) -> Option<Vec3> {
    let h = |p: Vec3| ground.height(p.x, -p.z);
    let (mut t0, step, max_t) = (0.0f32, 4.0f32, 30000.0f32);
    let mut t1 = step;
    while t1 < max_t {
        if (origin + dir * t1).y <= h(origin + dir * t1) {
            for _ in 0..16 {
                let tm = (t0 + t1) * 0.5;
                let pm = origin + dir * tm;
                if pm.y <= h(pm) {
                    t1 = tm;
                } else {
                    t0 = tm;
                }
            }
            return Some(origin + dir * t1);
        }
        t0 = t1;
        t1 += step;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::retaliation_post_due;

    /// `Player.cpp:704-714`: the mode is posted only on a whole logic second and only when the client's
    /// option differs from the player's logical mode.
    #[test]
    fn retaliation_is_posted_on_a_whole_second_only_when_it_differs() {
        assert!(!retaliation_post_due(1, true, false), "not a whole second");
        assert!(!retaliation_post_due(29, true, false));
        assert!(retaliation_post_due(0, true, false));
        assert!(retaliation_post_due(30, false, true), "turning it off is posted too");
        assert!(!retaliation_post_due(30, true, true), "equal modes post nothing");
        assert!(!retaliation_post_due(30, false, false));
    }
}
