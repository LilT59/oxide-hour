//! Containment (enter/exit, capacity, kind masks, container death), synthetic data only.

use std::sync::Arc;
use zr_sim::contain::{ContainKind, ContainTemplate, GarrisonFirePoints};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(100, 100, 0)
}

fn contain(kind: ContainKind, capacity: i32) -> ContainTemplate {
    ContainTemplate {
        kind,
        capacity,
        allow_inside_kind_of: None, // all allowed
        forbid_inside_kind_of: 0,
        allow_allies_inside: true,
        allow_enemies_inside: true,
        allow_neutral_inside: true,
        passengers_allowed_to_fire: false,
        damage_percentage_to_units: 0.0,
        number_of_exit_paths: 1,
        exit_delay: 0,
        door_open_time: 1,
        scatter_nearby_on_exit: true,
        orient_like_container_on_exit: false,
        reset_mood_check_on_exit: false,
        health_regen: 0.0,
        do_i_heal_objects: false,
        heal_frames: 0.0,
        initial_payload: Vec::new(),
        tunnel_frames_for_full_heal: 1.0,
        is_enclosing_container: true,
        fire_points: GarrisonFirePoints::default(),
        passengers_in_turret: false,
        weapon_bonus_passed_to_passengers: false,
        immune_to_clear_building_attacks: false,
    }
}

fn transport(slots: i32, contain: ContainTemplate) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Transport");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 10.0;
    t.max_health = 200.0;
    t.initial_health = 200.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    let mut c = contain;
    c.kind = ContainKind::Transport;
    c.capacity = slots;
    t.contain = Some(Arc::new(c));
    t
}

fn infantry(slot_count: u32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Infantry");
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    t.geometry.major_radius = 4.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 60.0, 120.0, 180.0));
    t.transport_slot_count = slot_count;
    t
}

#[test]
fn enter_and_exit_a_transport() {
    let mut g = GameLogic::new(flat(), 1);
    let c = g.create_object(&Arc::new(transport(2, contain(ContainKind::Transport, 2))), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let u = g.create_object(&Arc::new(infantry(1)), Coord3D::new(240.0, 200.0, 0.0), 0.0, 0);
    g.submit(0, Command::Enter { ids: vec![u], container: c });
    g.run_frames(200);
    assert!(g.is_contained(u), "the infantry should have boarded");
    assert_eq!(g.contained_units(c), &[u]);
    // A contained unit is pinned to the container.
    assert_eq!(g.find_object(u).unwrap().position, g.find_object(c).unwrap().position);
    assert_eq!(g.container_of(u), Some(c));

    g.submit(0, Command::Exit { container: c, ids: vec![u] });
    g.run_frames(3);
    assert!(!g.is_contained(u));
    assert!(g.contained_units(c).is_empty());
    // `OpenContain::exitObjectViaDoor` (OpenContain.cpp:1073) sets the `ExitStart` bone position. This fixture's model has
    // no exit bones, so the bone reads as the container's own position (`Object::getSingleLogicalBonePosition`,
    // Object.cpp:6115-6122): the unit starts on the centre in `AI_FOLLOW_PATH` and walks out along the exit path.
    let gap = g.find_object(u).unwrap().position.dist_2d(&g.find_object(c).unwrap().position);
    assert!(gap < 2.0, "the unit starts at the container's position, gap {gap}");
    assert_eq!(g.find_object(u).unwrap().ai.machine.current, zr_sim::ai_states::AiStateType::FollowPath);
    g.run_frames(120);
    let gap = g.find_object(u).unwrap().position.dist_2d(&g.find_object(c).unwrap().position);
    assert!(gap > 5.0, "the unit walks out of the transport, gap {gap}");
}

#[test]
fn capacity_is_counted_in_slots() {
    let mut g = GameLogic::new(flat(), 1);
    let c = g.create_object(&Arc::new(transport(1, contain(ContainKind::Transport, 1))), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let u1 = g.create_object(&Arc::new(infantry(1)), Coord3D::new(230.0, 200.0, 0.0), 0.0, 0);
    let u2 = g.create_object(&Arc::new(infantry(1)), Coord3D::new(230.0, 210.0, 0.0), 0.0, 0);
    g.submit(0, Command::Enter { ids: vec![u1, u2], container: c });
    g.run_frames(200);
    // The engine has no ordering guarantee beyond the sleepy heap: both AIEnter states check capacity
    // every update (`AIStates.cpp:6337`), so exactly one of the two gets the single slot and the other
    // gives up. (The old port re-checked capacity in a global proximity sweep, which made the first
    // ordered unit always win; that sweep is not a contain module's `update`.)
    assert_eq!(g.contained_units(c).len(), 1, "only one slot is available");
    let entered = g.contained_units(c)[0];
    assert!(entered == u1 || entered == u2);
    assert!(!g.is_contained(if entered == u1 { u2 } else { u1 }), "the loser stays outside");
}

#[test]
fn forbidden_kind_cannot_enter() {
    let mut g = GameLogic::new(flat(), 1);
    let mut c = contain(ContainKind::Transport, 2);
    c.allow_inside_kind_of = Some(kind_of::VEHICLE); // infantry not allowed
    let c = g.create_object(&Arc::new(transport(2, c)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let u = g.create_object(&Arc::new(infantry(1)), Coord3D::new(230.0, 200.0, 0.0), 0.0, 0);
    g.submit(0, Command::Enter { ids: vec![u], container: c });
    g.run_frames(120);
    assert!(!g.is_contained(u), "an infantry must not enter a vehicle-only transport");
    assert!(g.find_object(u).unwrap().enter_target.is_none(), "the enter order is dropped");
}

#[test]
fn container_death_damages_and_ejects_passengers() {
    let mut g = GameLogic::new(flat(), 1);
    let mut c = contain(ContainKind::Transport, 2);
    c.damage_percentage_to_units = 1.0; // 100% of max health on death
    let c = g.create_object(&Arc::new(transport(2, c)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let u = g.create_object(&Arc::new(infantry(1)), Coord3D::new(230.0, 200.0, 0.0), 0.0, 0);
    g.submit(0, Command::Enter { ids: vec![u], container: c });
    g.run_frames(200);
    assert!(g.is_contained(u));
    g.destroy_object(c);
    g.run_frames(3);
    assert!(g.find_object(u).is_none(), "a 100% damage-on-death transport kills its passenger");
}

/// `AIUpdateModuleData::getDisabledTypesToProcess` is `DISABLED_HELD`: a passenger's state machine
/// keeps running. A held rider is not mobile (`Object::isMobile`), so an order given to the passenger
/// (a path, `privateFollowPath`) is refused rather than carried out by the container.
#[test]
fn a_contained_passenger_refuses_a_path_order() {
    use zr_sim::ai_states::AiStateType;
    let mut g = GameLogic::new(flat(), 1);
    let c = g.create_object(&Arc::new(transport(2, contain(ContainKind::Transport, 2))), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    // The transport parks within its own tolerance of the goal; the passenger needs the same slack.
    let mut inf = infantry(1);
    inf.locomotor.as_mut().expect("legs").close_enough_dist = 20.0;
    let u = g.create_object(&Arc::new(inf), Coord3D::new(214.0, 200.0, 0.0), 0.0, 0);
    g.submit(0, Command::Enter { ids: vec![u], container: c });
    g.run_frames(200);
    assert!(g.is_contained(u));
    let goal = Coord3D::new(320.0, 200.0, 0.0);
    // `AIUpdateInterface::privateFollowPath` returns when `getObject()->isMobile()` is false
    // (`AIUpdate.cpp:3392`). `Object::isMobile` is false for a `DISABLED_HELD` rider
    // (`Object.cpp:2952`, `TransportContain.cpp:283`), so the passenger's path order is refused and no
    // state is set. The state machine still runs, but it stays idle.
    g.ai_follow_path(u, vec![goal], false);
    assert_eq!(g.find_object(u).unwrap().ai.machine.current, AiStateType::Idle, "a held rider refuses a path order");
    g.submit(0, Command::MoveTo { ids: vec![c], pos: goal });
    g.run_frames(600);
    assert!(g.is_contained(u));
    assert!(g.find_object(c).unwrap().position.dist_2d(&goal) < 30.0, "the transport reached the goal");
    assert_eq!(g.find_object(u).unwrap().ai.machine.current, AiStateType::Idle, "the refused order left the passenger idle");
}

/// `AIGroup::groupMoveToPosition` skips `DISABLED_HELD` occupants (`AIGroup.cpp:1688`) and
/// `getCenter` ignores them (`AIGroup.cpp:335`).
#[test]
fn a_group_move_does_not_order_contained_passengers() {
    use zr_sim::ai_states::AiStateType;
    let mut g = GameLogic::new(flat(), 1);
    let c = g.create_object(&Arc::new(transport(2, contain(ContainKind::Transport, 2))), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let u = g.create_object(&Arc::new(infantry(1)), Coord3D::new(230.0, 200.0, 0.0), 0.0, 0);
    g.submit(0, Command::Enter { ids: vec![u], container: c });
    g.run_frames(200);
    assert!(g.is_contained(u));
    g.submit(0, Command::MoveTo { ids: vec![c, u], pos: Coord3D::new(320.0, 200.0, 0.0) });
    g.update();
    assert_eq!(g.find_object(c).unwrap().ai.machine.current, AiStateType::MoveTo);
    assert_eq!(g.find_object(u).unwrap().ai.machine.current, AiStateType::Idle, "the passenger got no move order");
}
