//! Lane 165 (wave 19): `SpawnBehavior::createSpawn`'s `onEnslave` for a spawn that leaves through a spawn point or a
//! supply center's exit (SpawnBehavior.cpp:635-666, `SlavedUpdate::onEnslave` -> `startSlavedEffects`, SlavedUpdate.cpp:703-716).
//! The order in `createSpawn` is: `newObject`, `Player::onUnitCreated`, `setProducer`, the enslave loop, then the exit. AH7 s1
//! f1977 (the stinger sites' soldiers) drew the enslave `GameLogicRandomValue( 0, 2*PI )` (draw 1699, range 0..6) before the
//! next site's creation; the port drew nothing on these arms and was one draw behind from there on.

use std::sync::Arc;

use crate::ocl::{CreateObjectTemplate, OclNugget, OclTemplate};
use crate::production::{ProductionExit, ProductionTemplate, SpawnPointBone};
use crate::random::GameRandom;
use crate::template::{kind_of, ObjectTemplate};
use crate::{Coord3D, GameLogic, ObjectId, TerrainLogic};

/// A stinger-style soldier: no locomotor, so the exit leaves no path draws of its own. `slaved` gives it a `SlavedUpdate`.
fn soldier(slaved: bool) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("GLAInfantryStingerSoldier");
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    if slaved {
        t.slaved = Some(crate::modules2::SlavedTemplate {
            guard_max_range: 0,
            guard_wander_range: 0,
            attack_range: 0,
            repair_rate_per_second: 0.0,
            stay_on_same_layer_as_master: false,
        });
    }
    Arc::new(t)
}

/// A spawner with `exit` (and no production queue), at (100, 100) on flat ground.
fn site(exit: ProductionExit) -> (GameLogic, ObjectId) {
    let mut f = ObjectTemplate::new("Site");
    f.max_health = 1000.0;
    f.initial_health = 1000.0;
    f.production = Some(ProductionTemplate { exit, ..Default::default() });
    let mut g = GameLogic::new(TerrainLogic::flat(300, 300, 0), 1);
    let id = g.create_object(&Arc::new(f), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    (g, id)
}

/// The sim's random stream after one spawn through `exit` (`spawn_produced_unit`, the `createSpawn` exit).
fn stream_after_spawn(exit: ProductionExit, slaved: bool) -> GameRandom {
    let (mut g, site_id) = site(exit);
    let sid = g.spawn_produced_unit(site_id, &soldier(slaved));
    assert_ne!(sid, ObjectId::INVALID, "a spawn was made");
    g.rng_mut().clone()
}

/// A slaved spawn through a spawn point draws `GameLogicRandomValue(0, 2*PI)` (0..6) once more than a plain one, and
/// nothing else differs. Revert check: without the call in the `SpawnPoint` arm of `exit_factory_unit_ordered` the two
/// streams are equal.
#[test]
fn a_slaved_spawn_point_spawn_draws_its_pinned_direction_once() {
    let exit = || ProductionExit::SpawnPoint { bones: vec![SpawnPointBone { pos: (10.0, 0.0, 0.0), angle: 0.0 }] };
    let plain = stream_after_spawn(exit(), false);
    let slaved = stream_after_spawn(exit(), true);
    let mut expected = plain.clone();
    let _ = expected.value(0, 6);
    assert_eq!(slaved, expected, "`onEnslave` draws once after the creation hooks");
}

/// The same for a supply center's exit (`SupplyCenterProductionExitUpdate`), which `createSpawn` also takes. Revert
/// check: without the call in the `Create` arm of `exit_factory_unit_ordered` the two streams are equal.
#[test]
fn a_slaved_supply_center_spawn_draws_its_pinned_direction_once() {
    let exit = || ProductionExit::SupplyCenter {
        unit_create_point: (0.0, 0.0, 0.0),
        natural_rally_point: (10.0, 0.0, 0.0),
        grant_temporary_stealth: 0,
    };
    let plain = stream_after_spawn(exit(), false);
    let slaved = stream_after_spawn(exit(), true);
    let mut expected = plain.clone();
    let _ = expected.value(0, 6);
    assert_eq!(slaved, expected, "`onEnslave` draws once after the creation hooks");
}

/// An `ObjectCreationList` nugget that creates a slaved unit draws the pinned direction once, after the health draw
/// (`doStuffToObj`: `setInitialHealth` at ObjectCreationList.cpp:993, `onEnslave` at :1000-1005). The nugget here has no
/// dispositions, so the enslave draw is the last one. Revert check: without the call in `run_create_objects` the two
/// streams are equal.
#[test]
fn a_slaved_ocl_object_draws_its_pinned_direction_once() {
    let stream = |slaved: bool| {
        let mut g = GameLogic::new(TerrainLogic::flat(300, 300, 0), 1);
        let src = g.create_object(&Arc::new(ObjectTemplate::new("Source")), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let ocl = Arc::new(OclTemplate {
            name: "TestCreate".into(),
            nuggets: vec![OclNugget::CreateObject(CreateObjectTemplate {
                is_object: true,
                names_len: 1,
                objects: vec![Some(soldier(slaved))],
                count: 1,
                min_health: 1.0,
                max_health: 1.0,
                ..Default::default()
            })],
        });
        g.run_ocl_at(&ocl, src, None, false);
        assert_eq!(g.objects().filter(|o| o.template.name == "GLAInfantryStingerSoldier").count(), 1, "one unit made");
        g.rng_mut().clone()
    };
    let plain = stream(false);
    let slaved = stream(true);
    let mut expected = plain.clone();
    let _ = expected.value(0, 6);
    assert_eq!(slaved, expected, "the OCL's `onEnslave` draws `GameLogicRandomValue(0, 2*PI)` once more");
}
