//! Production exit updates (`QueueProductionExitUpdate`, `DefaultProductionExitUpdate`,
//! `SupplyCenterProductionExitUpdate`, `SpawnPointProductionExitUpdate`), synthetic data only.

use std::sync::Arc;

use zr_sim::disabled::DisabledType;
use zr_sim::modules2::SpawnTemplate;
use zr_sim::production::{ProductionExit, ProductionTemplate, QuantityModifier, SpawnPointBone};
use zr_sim::template::{LocomotorKind, LocomotorParams};
use zr_sim::{Coord3D, GameLogic, ObjectId, ObjectTemplate, TemplateSource, TerrainLogic};

/// A tiny name -> `Arc<ObjectTemplate>` source for the tests.
struct FixedStore(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for FixedStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

/// A factory whose production uses `exit`, building `quantity` soldiers at a time.
fn factory(exit: ProductionExit, quantity: i32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Factory");
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.production = Some(ProductionTemplate {
        quantity_modifiers: vec![QuantityModifier { template_name: "Soldier".into(), quantity }],
        exit,
        ..Default::default()
    });
    t
}

fn soldier(frames: u32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Soldier");
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.build_cost = 0;
    t.build_time_frames = frames;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 30.0, 100.0, 100.0, 360.0));
    t
}

/// A world with `f` at the origin facing +X, owned by player 0, and a `Soldier` template.
fn world(f: ObjectTemplate, frames: u32) -> (GameLogic, ObjectId) {
    world_with_unit(f, soldier(frames))
}

/// [`world`] with `unit` as the `Soldier` template.
fn world_with_unit(f: ObjectTemplate, unit: ObjectTemplate) -> (GameLogic, ObjectId) {
    let f = Arc::new(f);
    let mut g = GameLogic::new(TerrainLogic::flat(300, 300, 0), 1);
    g.add_player(0, 1000);
    g.set_template_source(Arc::new(FixedStore(vec![f.clone(), Arc::new(unit)])));
    let id = g.create_object(&f, Coord3D::new(0.0, 0.0, 0.0), 0.0, 0);
    g.run_frames(1); // the factory's first `ProductionUpdate` is frame 1 (GameLogic.cpp:4018-4045)
    (g, id)
}

fn soldiers(g: &GameLogic) -> Vec<ObjectId> {
    g.objects().filter(|o| o.template.name == "Soldier").map(|o| o.id).collect()
}

fn near(a: Coord3D, b: (f32, f32)) -> bool {
    (a.x - b.0).abs() < 0.01 && (a.y - b.1).abs() < 0.01
}

/// The exit path the newest soldier was given (`ai.machine.goal_path`).
fn exit_path(g: &GameLogic, id: ObjectId) -> Vec<Coord3D> {
    g.find_object(id).unwrap().ai.machine.goal_path.clone()
}

fn queue(create: (f32, f32, f32), natural: (f32, f32, f32), delay: u32, burst: u32) -> ProductionExit {
    ProductionExit::Queue {
        unit_create_point: create,
        natural_rally_point: natural,
        exit_delay_frames: delay,
        initial_burst: burst,
        allow_airborne_creation: false,
    }
}

#[test]
fn queue_burst_units_leave_together_then_the_delay_spaces_the_rest() {
    // `InitialBurst` 2: the first two leave on the same frame (`isFreeToExit` while bursting). The
    // third waits out `ExitDelay` (10) after the last burst unit. `QueueProductionExitUpdate` and
    // `ProductionUpdate` are separate heap slots with equal keys every frame, so their order inside a
    // frame is the heap's (GameLogic.cpp:3851-3905): on the burst frame production runs first (the exit
    // counts 9 left after it), on the frame the delay runs out the exit runs first and frees the door
    // before production, so the third leaves on the 10th frame, not the 11th.
    let (mut g, f) = world(factory(queue((10.0, 0.0, 0.0), (100.0, 0.0, 0.0), 10, 2), 3), 1);
    assert!(g.queue_unit(0, f, "Soldier"));
    g.run_frames(1);
    assert_eq!(soldiers(&g).len(), 2, "two burst units leave on the first frame");
    g.run_frames(8);
    assert_eq!(soldiers(&g).len(), 2, "the third waits out the exit delay");
    g.run_frames(1);
    assert_eq!(soldiers(&g).len(), 3, "the third leaves once the delay has run out");
}

#[test]
fn queue_delay_keeps_running_while_the_factory_is_idle() {
    // `QueueProductionExitUpdate::update` runs every frame, so a unit queued after the delay has run out
    // leaves the moment it is built. (A cooldown that only counts while producing would hold it back.)
    let (mut g, f) = world(factory(queue((10.0, 0.0, 0.0), (100.0, 0.0, 0.0), 5, 0), 1), 1);
    assert!(g.queue_unit(0, f, "Soldier"));
    g.run_frames(1);
    assert_eq!(soldiers(&g).len(), 1);
    g.run_frames(20);
    assert!(g.queue_unit(0, f, "Soldier"));
    g.run_frames(1);
    assert_eq!(soldiers(&g).len(), 2, "the exit is free again; nothing is left to wait for");
}

#[test]
fn queue_airborne_create_point_keeps_its_height_only_when_allowed() {
    // A create point 50 above the ground is clamped to the ground unless `AllowAirborneCreation`. The unit
    // has no locomotor here, so nothing moves it between its creation and the check.
    let (mut g, f) = world_with_unit(factory(queue((10.0, 0.0, 50.0), (100.0, 0.0, 0.0), 0, 0), 1), statue(1));
    assert!(g.queue_unit(0, f, "Soldier"));
    g.run_frames(1);
    let s = soldiers(&g)[0];
    assert!(g.find_object(s).unwrap().position.z.abs() < 0.01, "clamped to the ground");

    let mut allowed = factory(queue((10.0, 0.0, 50.0), (100.0, 0.0, 0.0), 0, 0), 1);
    if let Some(ProductionExit::Queue { allow_airborne_creation, .. }) = allowed.production.as_mut().map(|p| &mut p.exit) {
        *allow_airborne_creation = true;
    }
    let (mut g, f) = world_with_unit(allowed, statue(1));
    assert!(g.queue_unit(0, f, "Soldier"));
    g.run_frames(1);
    let s = soldiers(&g)[0];
    assert!((g.find_object(s).unwrap().position.z - 50.0).abs() < 0.01, "airborne creation keeps z");
}

/// A `Soldier` with no locomotor: it stays where it is created.
fn statue(frames: u32) -> ObjectTemplate {
    let mut t = soldier(frames);
    t.locomotor = None;
    t
}

#[test]
fn default_exit_path_starts_at_the_offset_natural_rally_point() {
    // `getNaturalRallyPoint(offset = TRUE)` pushes (100,0,0) out by 2 cells along +X: (120,0,0).
    let exit = ProductionExit::Default { unit_create_point: (10.0, 0.0, 0.0), natural_rally_point: (100.0, 0.0, 0.0) };
    let (mut g, f) = world(factory(exit, 1), 1);
    assert!(g.queue_unit(0, f, "Soldier"));
    g.run_frames(1);
    let s = soldiers(&g)[0];
    let path = exit_path(&g, s);
    assert_eq!(path.len(), 1, "no rally point: the natural rally point alone");
    assert!(near(path[0], (120.0, 0.0)), "offset natural point: {:?}", path[0]);
}

#[test]
fn default_exit_path_adds_the_rally_point_after_the_natural_one() {
    let exit = ProductionExit::Default { unit_create_point: (10.0, 0.0, 0.0), natural_rally_point: (100.0, 0.0, 0.0) };
    let (mut g, f) = world(factory(exit, 1), 1);
    assert!(g.set_rally_point(0, f, Coord3D::new(0.0, 150.0, 0.0)));
    assert!(g.queue_unit(0, f, "Soldier"));
    g.run_frames(1);
    let s = soldiers(&g)[0];
    let path = exit_path(&g, s);
    assert_eq!(path.len(), 2, "natural rally point, then the rally point: {path:?}");
    assert!(near(path[0], (120.0, 0.0)));
    // `adjustDestination` takes the rally point to a reachable cell (within one cell here).
    let d = path[1];
    assert!(((d.x - 0.0).powi(2) + (d.y - 150.0).powi(2)).sqrt() <= 10.0, "the rally point is adjusted: {d:?}");
}

#[test]
fn queue_exit_path_repeats_the_natural_point_without_a_rally_point() {
    // `QueueProductionExitUpdate.cpp:149`: "Double the destination to keep redguards from stacking".
    let (mut g, f) = world(factory(queue((10.0, 0.0, 0.0), (100.0, 0.0, 0.0), 0, 0), 1), 1);
    assert!(g.queue_unit(0, f, "Soldier"));
    g.run_frames(1);
    let s = soldiers(&g)[0];
    let path = exit_path(&g, s);
    assert_eq!(path.len(), 2, "{path:?}");
    // `QueueProductionExitUpdate.cpp:132-136`: the natural point is snapped to the new unit's grid cell
    // (`snapPosition`) before it is pushed, and :149-150 push that same point again. It stays in the cell the
    // unsnapped point is in, so it is within one cell (10 units) of (120, 0).
    assert_eq!(path[0], path[1], "{path:?}");
    assert!(((path[0].x - 120.0).powi(2) + path[0].y.powi(2)).sqrt() <= 10.0, "{path:?}");
}

#[test]
fn supply_center_exit_path_uses_the_unoffset_natural_point_and_the_rally_point_as_is() {
    // `SupplyCenterProductionExitUpdate::exitObjectViaDoor`: no 2-cell offset, rally point not adjusted.
    let exit = ProductionExit::SupplyCenter {
        unit_create_point: (10.0, 0.0, 0.0),
        natural_rally_point: (100.0, 0.0, 0.0),
        grant_temporary_stealth: 0,
    };
    let (mut g, f) = world(factory(exit, 1), 1);
    assert!(g.set_rally_point(0, f, Coord3D::new(0.0, 150.0, 0.0)));
    assert!(g.queue_unit(0, f, "Soldier"));
    g.run_frames(1);
    let s = soldiers(&g)[0];
    let path = exit_path(&g, s);
    assert_eq!(path.len(), 2, "{path:?}");
    assert!(near(path[0], (100.0, 0.0)), "{:?}", path[0]);
    assert!(near(path[1], (0.0, 150.0)), "{:?}", path[1]);
}

fn bone(x: f32, y: f32, angle: f32) -> SpawnPointBone {
    SpawnPointBone { pos: (x, y, 0.0), angle }
}

#[test]
fn spawn_point_units_stand_at_their_bones_held_until_a_spawn_point_frees() {
    // Two bones, three soldiers: the third waits in the queue until a bone is free again.
    let exit = ProductionExit::SpawnPoint { bones: vec![bone(10.0, 0.0, 0.0), bone(0.0, 10.0, 0.5)] };
    let (mut g, f) = world(factory(exit, 3), 1);
    assert!(g.queue_unit(0, f, "Soldier"));
    g.run_frames(1);
    let first_two = soldiers(&g);
    assert_eq!(first_two.len(), 2, "one unit per spawn point");
    let a = g.find_object(first_two[0]).unwrap();
    let b = g.find_object(first_two[1]).unwrap();
    assert!(near(a.position, (10.0, 0.0)) && a.angle.abs() < 1e-6, "bone 0: {:?} {}", a.position, a.angle);
    assert!(near(b.position, (0.0, 10.0)) && (b.angle - 0.5).abs() < 1e-6, "bone 1: {:?} {}", b.position, b.angle);
    assert!(a.is_disabled_by_type(DisabledType::Held) && b.is_disabled_by_type(DisabledType::Held), "held in place");

    g.run_frames(10);
    assert_eq!(soldiers(&g).len(), 2, "no third unit while both bones are occupied");
    assert_eq!(g.find_object(f).unwrap().production.as_ref().unwrap().queue.len(), 1, "the third is still building");

    // A dead occupant frees its bone (`revalidateOccupiers`).
    g.destroy_object(first_two[0]);
    g.run_frames(2);
    let now = soldiers(&g);
    assert_eq!(now.len(), 2, "the dead soldier is gone and the third took its bone");
    assert!(!now.contains(&first_two[0]));
    let third = g.objects().find(|o| o.template.name == "Soldier" && o.id != first_two[1]).unwrap();
    assert!(near(third.position, (10.0, 0.0)), "the freed bone 0 is reused: {:?}", third.position);
    assert!(g.find_object(f).unwrap().production.as_ref().unwrap().queue.is_empty());
}

#[test]
fn spawn_point_exit_without_bones_never_releases_a_unit() {
    let (mut g, f) = world(factory(ProductionExit::SpawnPoint { bones: Vec::new() }, 1), 1);
    assert!(g.queue_unit(0, f, "Soldier"));
    g.run_frames(5);
    assert!(soldiers(&g).is_empty(), "no spawn point, so the exit is never free");
    assert_eq!(g.find_object(f).unwrap().production.as_ref().unwrap().queue.len(), 1);
}

#[test]
fn spawn_behavior_slaves_take_spawn_points_and_refill_after_a_death() {
    // A spawner with one spawn point and `SpawnNumber` 2 (a stinger site): the first slave is held at the
    // bone, the second waits, and it takes the bone once the first dies.
    let mut site = factory(ProductionExit::SpawnPoint { bones: vec![bone(10.0, 0.0, 0.0)] }, 1);
    site.name = "Site".into();
    site.production.as_mut().unwrap().quantity_modifiers.clear();
    site.spawn = Some(SpawnTemplate {
        spawn_number: 2,
        spawn_replace_delay: 0,
        one_shot: false,
        spawn_template_names: vec!["Soldier".into()],
        spawned_require_spawner: false,
        initial_burst: 0,
        aggregate_health: false,
    });
    // A `SlavedUpdate` is what `SpawnBehavior::createSpawn` enslaves (`SpawnBehavior.cpp:658-666`): without one the
    // unit has only its producer (`:655`), and `Object::master` would stay unset.
    let mut soldier = soldier(1);
    soldier.slaved = Some(zr_sim::modules2::SlavedTemplate {
        guard_max_range: 0,
        guard_wander_range: 0,
        attack_range: 0,
        repair_rate_per_second: 0.0,
        stay_on_same_layer_as_master: false,
    });
    let (mut g, site_id) = world_with_unit(site, soldier);
    g.run_frames(1);
    let slaves = soldiers(&g);
    assert_eq!(slaves.len(), 1, "one spawn point, so one slave for now");
    let s = g.find_object(slaves[0]).unwrap();
    assert_eq!(s.master, Some(site_id));
    assert!(near(s.position, (10.0, 0.0)) && s.is_disabled_by_type(DisabledType::Held));

    g.destroy_object(slaves[0]);
    g.run_frames(16);
    let slaves = soldiers(&g);
    assert_eq!(slaves.len(), 1, "the waiting slave takes the freed spawn point on the next spawner update");
    assert!(g.find_object(slaves[0]).unwrap().is_disabled_by_type(DisabledType::Held));
}
