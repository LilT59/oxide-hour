//! Death pipeline and OCL execution (synthetic data, no game install).

use std::sync::Arc;

use zr_sim::body::{DamageInfo, DamageType};
use zr_sim::damage::DeathType;
use zr_sim::die::{DieMux, DieModuleTemplate, DieTemplate, SlowDeathTemplate};
use zr_sim::ocl::{CreateObjectTemplate, OclNugget, OclTemplate};
use zr_sim::physics::PhysicsTemplate;
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn thing(name: &str) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
    t.max_health = 300.0;
    t.initial_health = 300.0;
    Arc::new(t)
}

fn create_nugget(objects: Vec<Arc<ObjectTemplate>>, count: i32) -> CreateObjectTemplate {
    let names_len = objects.len();
    CreateObjectTemplate {
        is_object: true,
        names_len,
        objects: objects.into_iter().map(Some).collect(),
        count,
        offset: (0.0, 0.0, 0.0),
        disposition: 0,
        disposition_intensity: 0.0,
        spin_rate: -1.0,
        yaw_rate: -1.0,
        roll_rate: -1.0,
        pitch_rate: -1.0,
        min_force_pitch: 0.0,
        max_force_pitch: 0.0,
        min_force_magnitude: 0.0,
        max_force_magnitude: 0.0,
        orient_in_force_direction: false,
        spread_formation: false,
        min_distance_a_formation: 0.0,
        min_distance_b_formation: 0.0,
        max_distance_formation: 0.0,
        min_health: 1.0,
        max_health: 1.0,
        requires_live_player: false,
    }
}

fn ocl_create(objects: Vec<Arc<ObjectTemplate>>, count: i32) -> Arc<OclTemplate> {
    Arc::new(OclTemplate {
        name: "OCL_Test".into(),
        nuggets: vec![OclNugget::CreateObject(create_nugget(objects, count))],
    })
}

fn normal_only() -> DieMux {
    DieMux { death_types: 1 << (DeathType::Normal as u32), veterancy_levels: u32::MAX, exempt_status: 0, required_status: 0 }
}

fn slow_death(probability: i32, delay: u32, initial_ocl: Option<Arc<OclTemplate>>) -> DieTemplate {
    let mut ocl: [Vec<Arc<OclTemplate>>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    if let Some(o) = initial_ocl {
        ocl[0].push(o);
    }
    DieTemplate::SlowDeath(Arc::new(SlowDeathTemplate {
        probability_modifier: probability,
        modifier_bonus_per_overkill_percent: 0.0,
        sink_rate: 0.0,
        sink_delay: 0,
        sink_delay_variance: 0,
        destruction_delay: delay,
        destruction_delay_variance: 0,
        destruction_altitude: -10.0,
        fx: [Vec::new(), Vec::new(), Vec::new()],
        ocl,
        weapons: [Vec::new(), Vec::new(), Vec::new()],
        fling_force: 0.0,
        fling_force_variance: 0.0,
        fling_pitch: 0.0,
        fling_pitch_variance: 0.0,
        special: None,
    }))
}

fn kill(g: &mut GameLogic, id: zr_sim::ObjectId, kind: DamageType, death: DeathType) {
    g.damage_object_with_death(id, DamageInfo { source: zr_sim::ObjectId::INVALID, amount: 9999.0, kind }, death);
}

#[test]
fn die_mux_filters_by_death_type() {
    let spawned = thing("Spawned");
    let ocl = ocl_create(vec![spawned.clone()], 1);

    let build = || {
        let mut t = ObjectTemplate::new("Source");
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.die = vec![DieModuleTemplate { mux: normal_only(), kind: DieTemplate::CreateObject { ocl: Some(ocl.clone()), transfer_previous_health: false, transfer_selection: false } }];
        Arc::new(t)
    };

    // NORMAL: the module applies, so its OCL runs (and the source falls back to destroyed).
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 0);
    let id = g.create_object(&build(), Coord3D::new(10.0, 10.0, 0.0), 0.0, 7);
    kill(&mut g, id, DamageType::Explosion, DeathType::Normal);
    g.update();
    let spawned: Vec<_> = g.objects().filter(|o| o.template.name == "Spawned").collect();
    assert_eq!(spawned.len(), 1);
    assert_eq!(spawned[0].player, 7, "the spawned object inherits the source's owner");

    // CRUSHED: the module does not apply, so nothing is spawned.
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 0);
    let id = g.create_object(&build(), Coord3D::new(10.0, 10.0, 0.0), 0.0, 7);
    kill(&mut g, id, DamageType::Crush, DeathType::Crushed);
    g.update();
    assert_eq!(g.objects().filter(|o| o.template.name == "Spawned").count(), 0);
}

#[test]
fn destroy_die_destroys_immediately() {
    let mut t = ObjectTemplate::new("Source");
    t.max_health = 50.0;
    t.initial_health = 50.0;
    t.die = vec![DieModuleTemplate { mux: normal_only(), kind: DieTemplate::Destroy }];
    let t = Arc::new(t);

    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 0);
    let id = g.create_object(&t, Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    kill(&mut g, id, DamageType::Explosion, DeathType::Normal);
    // `destroyObject` is synchronous (the object is hidden at once; the frame removes it).
    assert!(g.find_object(id).is_none());
    g.update();
    assert_eq!(g.object_count(), 0);
}

#[test]
fn ocl_create_object_count_and_owner() {
    let spawned = thing("Spawned");
    let ocl = ocl_create(vec![spawned.clone()], 3);
    let mut t = ObjectTemplate::new("Source");
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.die = vec![DieModuleTemplate { mux: normal_only(), kind: DieTemplate::CreateObject { ocl: Some(ocl), transfer_previous_health: false, transfer_selection: false } }];
    let t = Arc::new(t);

    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 0);
    let id = g.create_object(&t, Coord3D::new(30.0, 30.0, 0.0), 0.0, 4);
    kill(&mut g, id, DamageType::Explosion, DeathType::Normal);
    g.update();
    let spawned: Vec<_> = g.objects().filter(|o| o.template.name == "Spawned").collect();
    assert_eq!(spawned.len(), 3);
    assert!(spawned.iter().all(|o| o.player == 4));
}

#[test]
fn slow_death_weighted_choice_is_deterministic() {
    let a = thing("A");
    let b = thing("B");
    let run = || {
        let mut t = ObjectTemplate::new("Source");
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.die = vec![
            DieModuleTemplate { mux: normal_only(), kind: slow_death(50, 30, Some(ocl_create(vec![a.clone()], 1))) },
            DieModuleTemplate { mux: normal_only(), kind: slow_death(50, 60, Some(ocl_create(vec![b.clone()], 1))) },
        ];
        let t = Arc::new(t);
        let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 12345);
        let id = g.create_object(&t, Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
        kill(&mut g, id, DamageType::Explosion, DeathType::Normal);
        g.update();
        let spawned: Vec<String> = g.objects().map(|o| o.template.name.clone()).collect();
        (spawned, g.crc())
    };
    let (s1, c1) = run();
    let (s2, c2) = run();
    assert_eq!(s1, s2, "the weighted choice must be deterministic");
    assert_eq!(c1, c2);
    // Exactly one of the two SlowDeath modules ran, so exactly one of A/B was spawned.
    let has_a = s1.iter().any(|n| n == "A");
    let has_b = s1.iter().any(|n| n == "B");
    assert!(has_a ^ has_b, "exactly one module should run: {s1:?}");
}

#[test]
fn slow_death_single_module_delays_destruction() {
    let mut t = ObjectTemplate::new("Source");
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.die = vec![DieModuleTemplate { mux: normal_only(), kind: slow_death(1, 10, None) }];
    let t = Arc::new(t);
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 0);
    let id = g.create_object(&t, Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    kill(&mut g, id, DamageType::Explosion, DeathType::Normal);
    // The object lingers (dead) until the destruction frame.
    assert!(g.find_object(id).is_some());
    assert!(g.find_object(id).unwrap().is_dead());
    g.run_frames(9);
    assert!(g.find_object(id).is_some(), "destroyed before the destruction delay");
    g.run_frames(2);
    assert!(g.find_object(id).is_none(), "should be destroyed after the delay");
}

#[test]
fn slow_death_fling_applies_force_and_leaves_the_ground() {
    // `SlowDeathBehavior::beginSlowDeath`'s fling block (`SlowDeathBehavior.cpp:265`): the object is
    // freed (`ALLOW_TO_FALL`), lifted above the ground, and a random force flings it.
    let sd = SlowDeathTemplate {
        probability_modifier: 1,
        modifier_bonus_per_overkill_percent: 0.0,
        sink_rate: 0.0,
        sink_delay: 0,
        sink_delay_variance: 0,
        destruction_delay: 600,
        destruction_delay_variance: 0,
        destruction_altitude: -10.0,
        fx: [Vec::new(), Vec::new(), Vec::new()],
        ocl: [Vec::new(), Vec::new(), Vec::new()],
        weapons: [Vec::new(), Vec::new(), Vec::new()],
        fling_force: 30.0,
        fling_force_variance: 0.0,
        fling_pitch: 1.0,
        fling_pitch_variance: 0.0,
        special: None,
    };
    let mut t = ObjectTemplate::new("Flung");
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 3.0;
    t.physics = Some(PhysicsTemplate { mass: 1.0, ..PhysicsTemplate::default() });
    t.die = vec![DieModuleTemplate { mux: normal_only(), kind: DieTemplate::SlowDeath(Arc::new(sd)) }];
    let t = Arc::new(t);

    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 7);
    let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    kill(&mut g, id, DamageType::Explosion, DeathType::Normal);
    // The fling force is applied this frame; the object's first physics update is one frame after
    // creation (`PhysicsUpdate.cpp:231-233`), so the force integrates on the next one.
    g.update();
    g.update();
    let p = g.find_object(id).unwrap().physics.as_ref().unwrap();
    assert!(p.allow_to_fall, "the fling frees the body to fall");
    assert!(p.velocity.z > 0.0, "the upward fling force integrated into velocity: {:?}", p.velocity);
    g.run_frames(12);
    let o = g.find_object(id).unwrap();
    assert!(o.position.z > 0.5, "the body is airborne: z {}", o.position.z);
}

#[test]
fn ocl_create_disposition_send_it_up_applies_force() {
    // `doStuffToObj`'s `SEND_IT_UP` disposition (`ObjectCreationList.cpp:1175`) applies an upward
    // physics force to the created object.
    let mut debris = ObjectTemplate::new("Debris");
    debris.max_health = 50.0;
    debris.initial_health = 50.0;
    debris.geometry.major_radius = 2.0;
    debris.physics = Some(PhysicsTemplate { mass: 1.0, ..PhysicsTemplate::default() });
    let debris = Arc::new(debris);
    let mut nugget = create_nugget(vec![debris.clone()], 1);
    nugget.disposition = zr_sim::ocl::disposition::SEND_IT_UP;
    nugget.disposition_intensity = 3.0;
    let ocl = Arc::new(OclTemplate { name: "OCL_Up".into(), nuggets: vec![OclNugget::CreateObject(nugget)] });
    let mut t = ObjectTemplate::new("Source");
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.die = vec![DieModuleTemplate { mux: normal_only(), kind: DieTemplate::CreateObject { ocl: Some(ocl), transfer_previous_health: false, transfer_selection: false } }];
    let t = Arc::new(t);
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 3);
    let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    kill(&mut g, id, DamageType::Explosion, DeathType::Normal);
    // The debris is created in this frame; its first physics update is the next one
    // (`PhysicsUpdate.cpp:231-233`), where the SEND_IT_UP force integrates.
    g.update();
    g.update();
    let spawned = g.objects().find(|o| o.template.name == "Debris").expect("debris created");
    let p = spawned.physics.as_ref().expect("debris physics");
    assert!(p.velocity.z > 0.0, "the SEND_IT_UP force integrated into velocity: {:?}", p.velocity);
}

#[test]
fn fx_list_die_records_an_event() {
    let mut t = ObjectTemplate::new("Source");
    t.max_health = 10.0;
    t.initial_health = 10.0;
    t.die = vec![DieModuleTemplate {
        mux: normal_only(),
        kind: DieTemplate::FxList { default_death_fx: Some("FX_Boom".into()), orient_to_object: true, initially_active: true },
    }];
    let t = Arc::new(t);
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 0);
    let id = g.create_object(&t, Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    kill(&mut g, id, DamageType::Explosion, DeathType::Normal);
    assert_eq!(g.fx_events().len(), 1);
    assert_eq!(g.fx_events()[0].name, "FX_Boom");
}
