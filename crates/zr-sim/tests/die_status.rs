//! Status-gated die modules (`DieMuxData::isDieApplicable`, `DieModule.cpp:66`) on synthetic data.
//!
//! The sim's status bits sit at the `ObjectStatusTypes.h` positions (`zr_sim::object::status`), and
//! the statuses the sim keeps in other fields (`UNDER_CONSTRUCTION`) reach the mux test.

use std::sync::Arc;

use zr_sim::body::{DamageInfo, DamageType};
use zr_sim::damage::DeathType;
use zr_sim::die::{DieMux, DieModuleTemplate, DieTemplate};
use zr_sim::modules2::{CrateCollideTemplate, CrateEffect};
use zr_sim::object::status;
use zr_sim::physics::PhysicsTemplate;
use zr_sim::template::{kind_of, GeometryShape, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{WeaponSetTemplate, WeaponTemplate, WEAPONSET_CARBOMB};
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, TemplateSource, TerrainLogic};

struct FixedStore(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for FixedStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

/// A mobile vehicle with physics and treads.
fn vehicle(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate::default());
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t
}

/// A vehicle with a primary weapon, so `Command::Attack` records an attack order.
fn armed_vehicle(name: &str) -> ObjectTemplate {
    let mut t = vehicle(name);
    let mut w = WeaponTemplate::new("Hit");
    w.primary_damage = 1.0;
    w.attack_range = 100.0;
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(w));
    t.combat.weapon_set = Some(set);
    t
}

/// A die module that destroys the object, applying only when the mux's status test passes.
fn destroy_when(required: u64, exempt: u64) -> DieModuleTemplate {
    let mux = DieMux { death_types: u32::MAX, veterancy_levels: u32::MAX, exempt_status: exempt, required_status: required };
    DieModuleTemplate { mux, kind: DieTemplate::Destroy }
}

fn kill(g: &mut GameLogic, id: ObjectId) {
    g.damage_object_with_death(
        id,
        DamageInfo { source: ObjectId::INVALID, amount: 9999.0, kind: DamageType::Explosion },
        DeathType::Normal,
    );
}

/// `DieMuxData::isDieApplicable` tests the whole status word, so bits past 31 must count
/// (`RIDER3` is bit 32 of `ObjectStatusTypes.h`).
#[test]
fn status_mux_reads_bits_above_31() {
    let required = DieMux { death_types: u32::MAX, veterancy_levels: u32::MAX, exempt_status: 0, required_status: status::RIDER3 };
    assert!(!required.is_applicable(DeathType::Normal, 0, 0));
    assert!(!required.is_applicable(DeathType::Normal, 0, status::RIDER2));
    assert!(required.is_applicable(DeathType::Normal, 0, status::RIDER3));
    assert!(required.is_applicable(DeathType::Normal, 0, status::RIDER3 | status::IS_CARBOMB));

    let exempt = DieMux { death_types: u32::MAX, veterancy_levels: u32::MAX, exempt_status: status::RIDER7, required_status: 0 };
    assert!(exempt.is_applicable(DeathType::Normal, 0, status::RIDER3));
    assert!(!exempt.is_applicable(DeathType::Normal, 0, status::RIDER3 | status::RIDER7));
}

/// `ConvertToCarBombCrateCollide` sets `OBJECT_STATUS_IS_CARBOMB` (bit 28). A die module with
/// `RequiredStatus = IS_CARBOMB` fires on the converted car only.
#[test]
fn required_is_carbomb_fires_only_on_a_carbomb() {
    let mut car = vehicle("Car");
    // `ConvertToCarBombCrateCollide::isValidToExecute` needs a carbomb weapon set on the target.
    car.weapon_sets = vec![(WEAPONSET_CARBOMB, WeaponSetTemplate::default())];
    car.die = vec![destroy_when(status::IS_CARBOMB, 0)];
    let car = Arc::new(car);
    let mut terrorist = armed_vehicle("Terrorist");
    terrorist.crate_collides = vec![CrateCollideTemplate { effect: CrateEffect::ConvertToCarBomb, ..Default::default() }];

    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let tid = g.create_object(&Arc::new(terrorist), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let bomb = g.create_object(&car, Coord3D::new(103.0, 100.0, 0.0), 0.0, 1);
    let plain = g.create_object(&car, Coord3D::new(50.0, 50.0, 0.0), 0.0, 1);
    // The terrorist's AI goal must be the target (`ai->getGoalObject() == other`).
    g.submit(0, Command::Attack { ids: vec![tid], target: bomb });
    g.run_frames(2);
    assert!(g.find_object(tid).is_none(), "the terrorist was consumed");
    assert_eq!(g.find_object(bomb).unwrap().player, 0, "the car is now a carbomb of player 0");

    kill(&mut g, plain);
    kill(&mut g, bomb);
    g.run_frames(2);
    assert!(g.find_object(plain).is_some(), "an ordinary car does not run the IS_CARBOMB destroy");
    assert!(g.find_object(bomb).is_none(), "the carbomb runs it");
}

/// `ConvertToHijackedVehicleCrateCollide` sets `OBJECT_STATUS_HIJACKED` (bit 10). An `ExemptStatus =
/// HIJACKED` die module (`EjectPilotDie`'s mux in shipped data) does not run on a hijacked vehicle.
#[test]
fn exempt_hijacked_skips_the_module_on_a_hijacked_vehicle() {
    let mut vehicle_t = vehicle("Vehicle");
    vehicle_t.die = vec![destroy_when(0, status::HIJACKED)];
    let vehicle_t = Arc::new(vehicle_t);
    let mut hijacker = armed_vehicle("Hijacker");
    hijacker.crate_collides = vec![CrateCollideTemplate { effect: CrateEffect::ConvertToHijackedVehicle, ..Default::default() }];

    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let hid = g.create_object(&Arc::new(hijacker), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let taken = g.create_object(&vehicle_t, Coord3D::new(103.0, 100.0, 0.0), 0.0, 1);
    let untouched = g.create_object(&vehicle_t, Coord3D::new(50.0, 50.0, 0.0), 0.0, 1);
    g.submit(0, Command::Attack { ids: vec![hid], target: taken });
    g.run_frames(2);
    assert_eq!(g.find_object(taken).unwrap().player, 0, "the vehicle defected");

    kill(&mut g, taken);
    kill(&mut g, untouched);
    g.run_frames(2);
    assert!(g.find_object(taken).is_some(), "the hijacked vehicle is exempt from the destroy");
    assert!(g.find_object(untouched).is_none(), "the ordinary vehicle runs it");
}

/// `DieMuxData` with `RequiredStatus = UNDER_CONSTRUCTION` (fake buildings in shipped data) applies
/// only while the building is still a construction site; the sim keeps that in `under_construction`.
#[test]
fn required_under_construction_follows_the_construction_site() {
    let mut dozer = vehicle("Dozer");
    // The dozer machine is the `DozerAIUpdate` module data; without it the engine's dozer never builds.
    dozer.dozer = Some(zr_sim::dozer::DozerTemplate::default());
    let mut site = ObjectTemplate::new("FakeBarracks");
    site.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    site.geometry.shape = GeometryShape::Box;
    site.geometry.major_radius = 15.0;
    site.geometry.minor_radius = 15.0;
    site.max_health = 500.0;
    site.initial_health = 500.0;
    site.build_cost = 300;
    site.build_time_frames = 10;
    site.die = vec![destroy_when(status::UNDER_CONSTRUCTION, 0)];

    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 1000);
    g.set_template_source(Arc::new(FixedStore(vec![Arc::new(dozer), Arc::new(site)])));
    let d = g.create_object(&g.find_template("Dozer").unwrap(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let under = g.construct(d, "FakeBarracks", Coord3D::new(200.0, 200.0, 0.0), 0.0).expect("construct");
    assert!(g.find_object(under).unwrap().under_construction);
    kill(&mut g, under);
    g.run_frames(2);
    assert!(g.find_object(under).is_none(), "a construction site runs the UNDER_CONSTRUCTION destroy");

    // The first dozer is still tied to the destroyed site, so a second dozer builds the finished one.
    let d2 = g.create_object(&g.find_template("Dozer").unwrap(), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
    let built = g.construct(d2, "FakeBarracks", Coord3D::new(300.0, 300.0, 0.0), 0.0).expect("construct");
    g.run_frames(600);
    assert!(!g.find_object(built).unwrap().under_construction, "finished");
    kill(&mut g, built);
    g.run_frames(2);
    assert!(g.find_object(built).is_some(), "a finished building does not run it");
}
