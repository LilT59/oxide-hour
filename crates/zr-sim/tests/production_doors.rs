//! `ProductionUpdate`'s doors for a factory (`updateDoors` and the door block of `update`) and an
//! airfield's hangar door (synthetic data, no game install). A factory's finished unit leaves only
//! once its door is fully open (`ProductionUpdate.cpp:771`); the door then waits open, closes, and
//! finishes closing.

use std::sync::Arc;

use zr_sim::condition::{DOOR_1_CLOSING, DOOR_1_OPENING, DOOR_1_WAITING_OPEN};
use zr_sim::jet_ai::DoorRt;
use zr_sim::production::{ProductionExit, ProductionTemplate};
use zr_sim::template::{collide_kind, kind_of, LocomotorKind, LocomotorParams};
use zr_sim::modules2::ParkingPlaceTemplate;
use zr_sim::{Coord3D, GameLogic, ObjectId, ObjectTemplate, TemplateSource, TerrainLogic};

struct FixedStore(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for FixedStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

/// `DoorOpeningTime 45`, `DoorWaitOpenTime 90`, `DoorCloseTime 45`, in frames.
fn production(num_door_animations: i32) -> ProductionTemplate {
    ProductionTemplate {
        exit: ProductionExit::Default { unit_create_point: (10.0, 0.0, 0.0), natural_rally_point: (100.0, 0.0, 0.0) },
        num_door_animations,
        door_opening_time: 45,
        door_wait_open_time: 90,
        door_close_time: 45,
        ..Default::default()
    }
}

/// A Command-Center-like factory with `num_door_animations` doors.
fn factory(num_door_animations: i32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Factory");
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.production = Some(production(num_door_animations));
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

/// A world with `factory` (player 0) and a `Soldier` that takes `frames` to build.
fn setup(factory_t: ObjectTemplate, frames: u32) -> (GameLogic, ObjectId) {
    let f = Arc::new(factory_t);
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    g.add_player(0, 1000);
    g.set_template_source(Arc::new(FixedStore(vec![f.clone(), Arc::new(soldier(frames))])));
    let id = g.create_object(&f, Coord3D::new(0.0, 0.0, 0.0), 0.0, 0);
    g.run_frames(1); // the factory's first `ProductionUpdate` is frame 1 (GameLogic.cpp:4018-4045)
    (g, id)
}

fn door(g: &GameLogic, factory: ObjectId) -> DoorRt {
    g.find_object(factory).expect("factory").production.as_ref().expect("production").door.clone()
}

fn soldiers(g: &GameLogic) -> usize {
    g.objects().filter(|o| o.template.name == "Soldier").count()
}

/// Runs one frame at a time until `cond` holds and returns how many frames that took.
fn step_until(g: &mut GameLogic, max: u32, cond: impl Fn(&GameLogic) -> bool) -> u32 {
    for n in 1..=max {
        g.run_frames(1);
        if cond(g) {
            return n;
        }
    }
    panic!("condition not reached within {max} frames");
}

#[test]
fn a_finished_unit_leaves_once_its_door_is_fully_open() {
    let (mut g, f) = setup(factory(1), 10);
    assert!(g.queue_unit(0, f, "Soldier"));

    // The unit completes on the tenth frame and its door starts opening that frame (`ProductionUpdate.cpp:723`).
    step_until(&mut g, 20, |g| door(g, f).opened_frame != 0);
    assert_eq!(soldiers(&g), 0, "the unit waits for its door");
    assert!(g.find_object(f).unwrap().model_conditions.test(DOOR_1_OPENING), "the opening condition is set");

    // `updateDoors` moves the door to waiting-open once `now - opened > DoorOpeningTime` (`:506`), and the
    // unit leaves that same frame: DoorOpeningTime + 1 frames after completion.
    let exit = step_until(&mut g, 100, |g| soldiers(g) == 1);
    assert_eq!(exit, 46, "the unit exits 45 + 1 frames after the door opened");
    assert_eq!(door(&g, f).wait_open_frame, g.frame() - 1, "the door is waiting open on the exit frame");
    assert!(g.find_object(f).unwrap().model_conditions.test(DOOR_1_WAITING_OPEN));
    assert!(!g.find_object(f).unwrap().model_conditions.test(DOOR_1_OPENING));
}

#[test]
fn the_door_waits_open_then_closes_then_finishes_closing() {
    let (mut g, f) = setup(factory(1), 10);
    assert!(g.queue_unit(0, f, "Soldier"));
    step_until(&mut g, 20, |g| door(g, f).opened_frame != 0);
    step_until(&mut g, 100, |g| soldiers(g) == 1);

    // `now - waitOpen > DoorWaitOpenTime (90)` starts the close (`:526`), the closing condition is
    // set, and the door is closed `DoorCloseTime (45)` later (`:546`).
    let to_closing = step_until(&mut g, 200, |g| door(g, f).closed_frame != 0);
    assert_eq!(to_closing, 91, "waiting open for 90 frames, then closing");
    assert!(g.find_object(f).unwrap().model_conditions.test(DOOR_1_CLOSING));
    assert_eq!(door(&g, f).wait_open_frame, 0);

    let to_closed = step_until(&mut g, 200, |g| door(g, f).closed_frame == 0);
    assert_eq!(to_closed, 46, "closing for 45 frames, then the door is shut");
    assert!(!g.find_object(f).unwrap().model_conditions.test(DOOR_1_CLOSING));
    assert!(!g.find_object(f).unwrap().model_conditions.any());
}

#[test]
fn a_factory_without_door_animations_exits_at_once() {
    // The door times are set but `NumDoorAnimations` is 0: `update` never waits for a door (`:771`).
    let (mut g, f) = setup(factory(0), 10);
    assert!(g.queue_unit(0, f, "Soldier"));
    let done = step_until(&mut g, 20, |g| soldiers(g) == 1);
    assert_eq!(done, 10, "the unit exits on the frame it completes");
    assert_eq!(door(&g, f), DoorRt::default(), "no door state without door animations");
}

#[test]
fn a_unit_queued_behind_one_leaves_as_soon_as_it_completes_through_the_open_door() {
    let (mut g, f) = setup(factory(1), 10);
    assert!(g.queue_unit(0, f, "Soldier"));
    assert!(g.queue_unit(0, f, "Soldier"));
    step_until(&mut g, 100, |g| soldiers(g) == 1);

    // The second unit starts its build as the first one leaves, and its door is still waiting open.
    let next = step_until(&mut g, 100, |g| soldiers(g) == 2);
    assert_eq!(next, 10, "the second unit exits when its own build time is up");
    assert_eq!(door(&g, f).opened_frame, 0, "the door was not reopened for the second unit");
}

#[test]
fn the_entry_holds_door_one_while_the_factory_door_opens() {
    // `ProductionUpdate::update` (`:710-716`) stores the door `reserveDoorForExit` returned on the
    // production entry (`setExitDoor`) and keeps it while the factory's door animation runs; the
    // snapshot's `Q door` prints `m_exitDoor` (`DOOR_1` = 0). The unit leaves only once the door is
    // open (`:771`), and `oneProductionSuccessful()` (`ProductionUpdate.h:89`) clears the door then.
    let (mut g, f) = setup(factory(1), 10);
    assert!(g.queue_unit(0, f, "Soldier"));
    step_until(&mut g, 20, |g| door(g, f).opened_frame != 0);
    let queue = &g.find_object(f).unwrap().production.as_ref().unwrap().queue;
    assert_eq!(queue.len(), 1, "the unit waits for the door");
    assert_eq!(queue[0].door, Some(0), "the reserved DOOR_1 is stored on the entry while it waits");
    step_until(&mut g, 100, |g| soldiers(g) == 1);
    assert!(g.find_object(f).unwrap().production.as_ref().unwrap().queue.is_empty(), "the entry left with the unit");
}

#[test]
fn an_airfield_holds_its_jet_for_the_door_opening_time_too() {
    let mut af = ObjectTemplate::new("Airfield");
    af.kind_of = kind_of::STRUCTURE;
    af.collide_kinds = collide_kind::FS_AIRFIELD;
    af.max_health = 1000.0;
    af.initial_health = 1000.0;
    af.geometry.major_radius = 60.0;
    af.geometry.minor_radius = 40.0;
    af.geometry.height = 10.0;
    af.production = Some(production(1));
    af.parking_place = Some(ParkingPlaceTemplate {
        num_rows: 1,
        num_cols: 1,
        has_runways: false,
        heal_amount_per_second: 0.0,
        approach_height: 0.0,
        landing_deck_height_offset: 0.0,
        park_in_hangars: false,
    });
    let (mut g, f) = setup(af, 5);
    assert!(g.queue_unit(0, f, "Soldier"));
    step_until(&mut g, 20, |g| g.find_object(f).unwrap().parking.as_ref().unwrap().doors[0].opened_frame != 0);
    assert_eq!(soldiers(&g), 0, "the airfield's door is still opening");
    let exit = step_until(&mut g, 100, |g| soldiers(g) == 1);
    assert_eq!(exit, 46, "the parked unit leaves 45 + 1 frames after its door opened");
}
