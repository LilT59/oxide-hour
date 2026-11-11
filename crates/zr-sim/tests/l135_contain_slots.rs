//! Lane 135: contain modules as sleepy-heap slots. `OpenContain` and every subclass derive from
//! `UpdateModule` (`OpenContain.h:80`); `registerObject` pushes one slot per INI module
//! (GameLogic.cpp:4013-4046), and each class's `update()` returns `UPDATE_SLEEP_NONE`
//! (`OpenContain.cpp:199-218`, `TransportContain.cpp:468-526`, `TunnelContain.cpp:552-595`,
//! `RiderChangeContain.cpp:363-379`). The per-frame logic these tests exercise used to run in global
//! sweeps (`contain.rs::update_contain`, `tunnel.rs::update_tunnel_heal`, `rider.rs::rider_update`).
//!
//! Synthetic data only.

use std::sync::Arc;

use zr_sim::body::DamageInfo;
use zr_sim::contain::{ContainKind, ContainTemplate, GarrisonFirePoints};
use zr_sim::damage::DamageType;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, TemplateSource, TerrainLogic};

struct Store(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for Store {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

fn contain(kind: ContainKind, capacity: i32) -> ContainTemplate {
    ContainTemplate {
        kind,
        capacity,
        allow_inside_kind_of: None,
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
        tunnel_frames_for_full_heal: 30.0,
        is_enclosing_container: true,
        fire_points: GarrisonFirePoints::default(),
        passengers_in_turret: false,
        weapon_bonus_passed_to_passengers: false,
        immune_to_clear_building_attacks: false,
    }
}

fn mobile(name: &str, c: ContainTemplate) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.max_health = 200.0;
    t.initial_health = 200.0;
    t.geometry.major_radius = 10.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t.contain = Some(Arc::new(c));
    t
}

fn infantry(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 4.0;
    t.transport_slot_count = 1;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 60.0, 120.0, 180.0));
    t
}

fn world(templates: Vec<ObjectTemplate>) -> GameLogic {
    let store = Store(templates.into_iter().map(Arc::new).collect());
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    g.add_player(0, 2000);
    g.set_template_source(Arc::new(store));
    g
}

fn damage(g: &mut GameLogic, id: ObjectId, amount: f32) {
    g.damage_object(id, DamageInfo { source: ObjectId::INVALID, amount, kind: DamageType::Unresistable });
}

/// `TransportContain::update` creates an `InitialPayload` once, on the module's first heap update
/// (`TransportContain.cpp:468-484`, `createPayload` `:434-462`), and never again. Revert check:
/// without the contain slot registration nothing runs and no passenger is created.
#[test]
fn the_transport_slot_creates_the_initial_payload_once() {
    let mut c = contain(ContainKind::Transport, 4);
    c.initial_payload = vec![("Ranger".into(), 2)];
    let mut g = world(vec![mobile("Humvee", c), infantry("Ranger")]);
    let id = g.create_object(&g.find_template("Humvee").expect("Humvee"), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    assert!(g.contained_units(id).is_empty());
    // The module is registered at the creation frame; `registerObject` clamps frame 0 to 1
    // (GameLogic.cpp:4018-4045), so its first update is the second `run_frames` step.
    g.run_frames(2);
    assert_eq!(g.contained_units(id).len(), 2, "the first heap update creates the payload");
    g.run_frames(10);
    assert_eq!(g.contained_units(id).len(), 2, "and never a second batch");
    assert_eq!(g.object_count(), 3, "humvee + two rangers");
}

/// `TransportContain::update`'s `HealthRegen%PerSec` loop heals every hurt passenger by
/// `maxHealth * percent / 100 * SECONDS_PER_LOGICFRAME_REAL` (`TransportContain.cpp:486-524`). Revert
/// check: remove the `Contain(Transport)` dispatch arm and the health does not change.
#[test]
fn the_transport_slot_heals_a_hurt_passenger_once_per_frame() {
    let mut c = contain(ContainKind::Transport, 2);
    c.health_regen = 100.0; // 100% of max health per second
    let mut g = world(vec![mobile("Humvee", c), infantry("Ranger")]);
    let truck = g.create_object(&g.find_template("Humvee").expect("Humvee"), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let ranger = g.create_object(&g.find_template("Ranger").expect("Ranger"), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.submit(0, Command::Enter { ids: vec![ranger], container: truck });
    g.run_frames(3);
    assert!(g.is_contained(ranger));
    damage(&mut g, ranger, 50.0);
    let before = g.find_object(ranger).expect("ranger").health();
    g.run_frames(1);
    let after = g.find_object(ranger).expect("ranger").health();
    assert!((after - before - 100.0 / 30.0).abs() < 1e-3, "one frame's regen, got {}", after - before);
}

/// `TunnelContain::update` calls `TunnelTracker::healObjects(framesForFullHeal)`
/// (`TunnelContain.cpp:552-593`, `TunnelTracker.cpp:333-336`): one slot per entrance, one heal per
/// update. Revert check: remove the `Contain(Tunnel)` dispatch arm and the passenger stays hurt.
#[test]
fn the_tunnel_slot_heals_the_network() {
    let mut t = ObjectTemplate::new("Tunnel");
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::SELECTABLE;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.geometry.major_radius = 20.0;
    t.contain = Some(Arc::new(contain(ContainKind::Tunnel, -1)));
    let mut g = world(vec![t, infantry("Ranger")]);
    g.set_rules(zr_sim::GameRules { max_tunnel_capacity: 2, ..zr_sim::GameRules::default() });
    let tunnel = g.create_object(&g.find_template("Tunnel").expect("Tunnel"), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let ranger = g.create_object(&g.find_template("Ranger").expect("Ranger"), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.submit(0, Command::Enter { ids: vec![ranger], container: tunnel });
    g.run_frames(5);
    assert_eq!(g.container_of(ranger), Some(tunnel));
    damage(&mut g, ranger, 50.0);
    let before = g.find_object(ranger).expect("ranger").health();
    g.run_frames(1);
    let after = g.find_object(ranger).expect("ranger").health();
    assert!((after - before - 100.0 / 30.0).abs() < 1e-3, "one tunnel heal, got {}", after - before);
}

/// `RiderChangeContain::update` topples a scuttled bike after `ScuttleDelay` frames
/// (`RiderChangeContain.cpp:363-376`). Revert check: remove the `Contain(RiderChange)` dispatch arm
/// and the bike stays alive.
#[test]
fn the_rider_change_slot_topples_a_scuttled_bike() {
    let mut c = contain(ContainKind::RiderChange, 1);
    c.allow_inside_kind_of = Some(kind_of::INFANTRY);
    let mut bike = mobile("Bike", c);
    bike.rider_change = Some(Arc::new(zr_sim::rider::RiderChangeTemplate {
        riders: vec![zr_sim::rider::RiderSlot {
            template_name: "Ranger".into(),
            model_condition: None,
            weapon_set_flag: 0,
            object_status: 0,
            locomotor_set: zr_sim::template::LocomotorSet::Normal,
            command_set: String::new(),
        }],
        scuttle_frames: 10,
        scuttle_state: None,
    }));
    let mut g = world(vec![bike, infantry("Ranger")]);
    let bike = g.create_object(&g.find_template("Bike").expect("Bike"), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let rider = g.create_object(&g.find_template("Ranger").expect("Ranger"), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.submit(0, Command::Enter { ids: vec![rider], container: bike });
    g.run_frames(3);
    assert_eq!(g.container_of(rider), Some(bike));
    g.submit(0, Command::Exit { container: bike, ids: vec![rider] });
    g.run_frames(3);
    assert!(!g.is_contained(rider), "the rider left and scuttled the bike");
    g.run_frames(10);
    assert!(g.find_object(bike).is_none_or(|o| o.is_dying()), "the scuttled bike toppled after its delay");
}
