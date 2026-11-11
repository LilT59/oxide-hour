//! Disabled types (`DisabledTypes.h`, `Object::setDisabled*`), synthetic data only.

use std::sync::Arc;

use zr_sim::damage::{DamageType, DeathType};
use zr_sim::disabled::DisabledType;
use zr_sim::production::{ProductionExit, ProductionTemplate};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::{body::DamageInfo, Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, TemplateSource, TerrainLogic};

struct FixedStore(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for FixedStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

fn vehicle(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t
}

fn flat() -> TerrainLogic {
    TerrainLogic::flat(200, 200, 0)
}

#[test]
fn disabled_until_expires_and_clears() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(vehicle("V")), Coord3D::new(0.0, 0.0, 0.0), 0.0, 0);
    g.set_disabled_until(id, DisabledType::Hacked, 5);
    assert!(g.find_object(id).unwrap().is_disabled_by_type(DisabledType::Hacked));
    assert_eq!(g.find_object(id).unwrap().get_disabled_until(DisabledType::Hacked), 5);
    g.run_frames(2);
    assert!(g.find_object(id).unwrap().is_disabled(), "still disabled at frame 2");
    g.run_frames(5);
    assert!(!g.find_object(id).unwrap().is_disabled(), "expired by frame 7");
    assert_eq!(g.find_object(id).unwrap().disabled_until(), 0);
}

#[test]
fn a_forever_disable_stays_until_cleared() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(vehicle("V")), Coord3D::new(0.0, 0.0, 0.0), 0.0, 0);
    g.set_disabled(id, DisabledType::Emp);
    g.run_frames(100);
    assert!(g.find_object(id).unwrap().is_disabled_by_type(DisabledType::Emp), "FOREVER never expires");
    assert!(g.clear_disabled(id, DisabledType::Emp));
    assert!(!g.clear_disabled(id, DisabledType::Emp), "a second clear is a no-op");
    assert!(!g.find_object(id).unwrap().is_disabled());
}

#[test]
fn crc_returns_to_baseline_when_the_disable_is_cleared() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(vehicle("V")), Coord3D::new(0.0, 0.0, 0.0), 0.0, 0);
    let baseline = g.crc();
    g.set_disabled(id, DisabledType::Hacked);
    assert_ne!(g.crc(), baseline, "an active disable is hashed");
    g.clear_disabled(id, DisabledType::Hacked);
    assert_eq!(g.crc(), baseline, "clearing restores the historical CRC");
}

#[test]
fn movement_stops_for_the_blocking_types() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(vehicle("V")), Coord3D::new(0.0, 0.0, 0.0), 0.0, 0);
    g.submit(0, Command::MoveTo { ids: vec![id], pos: Coord3D::new(150.0, 0.0, 0.0) });
    g.run_frames(5);
    let moved = g.find_object(id).unwrap().position;
    assert!(moved.x > 0.0, "the unit moved before being disabled");
    for t in [
        DisabledType::Paralyzed,
        DisabledType::Unmanned,
        DisabledType::Emp,
        DisabledType::Subdued,
        DisabledType::Hacked,
    ] {
        g.set_disabled(id, t);
        let before = g.find_object(id).unwrap().position;
        g.run_frames(5);
        let after = g.find_object(id).unwrap().position;
        assert_eq!(after, before, "{t:?} stops movement");
        assert!(g.clear_disabled(id, t));
    }
    g.run_frames(5);
    assert!(g.find_object(id).unwrap().position != moved, "movement resumes once cleared");
}

#[test]
fn kill_pilot_sets_unmanned_and_neutral() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(vehicle("V")), Coord3D::new(0.0, 0.0, 0.0), 0.0, 1);
    g.damage_object_with_death(id, DamageInfo { source: ObjectId::INVALID, amount: 50.0, kind: DamageType::KillPilot }, DeathType::Normal);
    let o = g.find_object(id).unwrap();
    assert!(o.is_disabled_by_type(DisabledType::Unmanned));
    assert!(o.unmanned());
    assert_eq!(o.health(), 100.0, "the snipe does no health damage");
}

#[test]
fn a_disabled_factory_pauses_production_but_held_does_not() {
    let mut f = vehicle("Factory");
    f.production = Some(ProductionTemplate {
        max_queue_entries: 9,
        quantity_modifiers: Vec::new(),
        exit: ProductionExit::Default { unit_create_point: (10.0, 0.0, 0.0), natural_rally_point: (100.0, 0.0, 0.0) },
        ..Default::default()
    });
    let f = Arc::new(f);
    let mut u = vehicle("Soldier");
    u.build_time_frames = 100;
    let u = Arc::new(u);
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 1000);
    g.set_template_source(Arc::new(FixedStore(vec![f.clone(), u])));
    let factory = g.create_object(&f, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    assert!(g.queue_unit(0, factory, "Soldier"));
    g.set_disabled(factory, DisabledType::Hacked);
    g.run_frames(4);
    assert_eq!(g.find_object(factory).unwrap().production.as_ref().unwrap().queue[0].frames, 0, "HACKED pauses the queue");
    assert!(g.clear_disabled(factory, DisabledType::Hacked));
    g.run_frames(4);
    assert!(g.find_object(factory).unwrap().production.as_ref().unwrap().queue[0].frames > 0, "cleared factory resumes");

    // HELD is in ProductionUpdate's DisabledTypesToProcess, so a contained factory keeps producing.
    g.set_disabled(factory, DisabledType::Held);
    let progressed = g.find_object(factory).unwrap().production.as_ref().unwrap().queue[0].frames;
    g.run_frames(3);
    assert!(g.find_object(factory).unwrap().production.as_ref().unwrap().queue[0].frames > progressed);
}

#[test]
fn powered_objects_are_underpowered_during_a_brownout() {
    let mut consumer = ObjectTemplate::new("Consumer");
    consumer.kind_of = kind_of::STRUCTURE | kind_of::POWERED;
    consumer.max_health = 100.0;
    consumer.initial_health = 100.0;
    consumer.energy_production = -5;
    let consumer = Arc::new(consumer);
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let id = g.create_object(&consumer, Coord3D::new(0.0, 0.0, 0.0), 0.0, 0);
    g.run_frames(1);
    assert!(g.find_object(id).unwrap().is_disabled_by_type(DisabledType::Underpowered), "brownout disables powered objects");

    let mut producer = ObjectTemplate::new("Producer");
    producer.kind_of = kind_of::STRUCTURE;
    producer.max_health = 100.0;
    producer.initial_health = 100.0;
    producer.energy_production = 10;
    g.create_object(&Arc::new(producer), Coord3D::new(50.0, 0.0, 0.0), 0.0, 0);
    g.run_frames(1);
    assert!(!g.find_object(id).unwrap().is_disabled_by_type(DisabledType::Underpowered), "power restores the object");
}

#[test]
fn able_to_attack_gates_subdued_and_portable_structures() {
    let mut g = GameLogic::new(flat(), 1);
    let tank = g.create_object(&Arc::new(vehicle("Tank")), Coord3D::new(0.0, 0.0, 0.0), 0.0, 0);
    assert!(g.is_able_to_attack(tank));
    g.set_disabled(tank, DisabledType::Subdued);
    assert!(!g.is_able_to_attack(tank), "SUBDUED silences a normal unit");
    g.clear_disabled(tank, DisabledType::Subdued);

    let mut stinger = vehicle("StingerSite");
    stinger.kind_of = kind_of::STRUCTURE | kind_of::PORTABLE_STRUCTURE | kind_of::SPAWNS_ARE_THE_WEAPONS;
    let site = g.create_object(&Arc::new(stinger), Coord3D::new(50.0, 0.0, 0.0), 0.0, 0);
    assert!(g.is_able_to_attack(site));
    g.set_disabled(site, DisabledType::Emp);
    assert!(!g.is_able_to_attack(site), "EMP silences a portable structure");
    g.clear_disabled(site, DisabledType::Emp);
    g.set_disabled(site, DisabledType::Hacked);
    assert!(!g.is_able_to_attack(site), "HACKED silences a portable structure");
}

#[test]
fn held_does_not_pause_special_powers_but_other_types_do() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(vehicle("V")), Coord3D::new(0.0, 0.0, 0.0), 0.0, 0);
    g.set_disabled(id, DisabledType::Held);
    assert!(!g.find_object(id).unwrap().special_powers_paused(), "HELD never pauses special powers");
    g.set_disabled(id, DisabledType::Hacked);
    assert!(g.find_object(id).unwrap().special_powers_paused(), "HACKED pauses special powers");
    g.clear_disabled(id, DisabledType::Hacked);
    assert!(!g.find_object(id).unwrap().special_powers_paused(), "HELD-only is still unpaused");
}

#[test]
fn an_unmanned_vehicle_cannot_crush() {
    let mut g = GameLogic::new(flat(), 1);
    let mut crusher = vehicle("Crusher");
    crusher.crush.crusher_level = 2;
    let a = g.create_object(&Arc::new(crusher), Coord3D::new(0.0, 0.0, 0.0), 0.0, 0);
    let mut victim = vehicle("Victim");
    victim.crush.crushable_level = 1;
    let b = g.create_object(&Arc::new(victim), Coord3D::new(5.0, 0.0, 0.0), 0.0, 1);
    assert!(g.can_crush_or_squish(a, b));
    g.set_disabled(a, DisabledType::Unmanned);
    assert!(!g.can_crush_or_squish(a, b), "an unmanned vehicle cannot crush");
}
