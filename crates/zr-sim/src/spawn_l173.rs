//! Lane 173 (wave 20): R19-7 — `SlavedUpdate::onEnslave` sets the slave's master
//! (`SlavedUpdate.cpp:88-91`, `startSlavedEffects`, `:703-716`: `m_slaver = slaver->getID()`), and the two engine
//! callers run it before the spawn's exit (`SpawnBehavior.cpp:657-666`) or right after creation
//! (`ObjectCreationList.cpp:1000-1005`).

use std::sync::Arc;

use crate::ocl::{CreateObjectTemplate, OclNugget, OclTemplate};
use crate::production::{ProductionExit, ProductionTemplate, SpawnPointBone};
use crate::template::{kind_of, ObjectTemplate};
use crate::{Coord3D, GameLogic, ObjectId, TerrainLogic};

fn soldier() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("GLAInfantryStingerSoldier");
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.slaved = Some(crate::modules2::SlavedTemplate {
        guard_max_range: 0,
        guard_wander_range: 0,
        attack_range: 0,
        repair_rate_per_second: 0.0,
        stay_on_same_layer_as_master: false,
    });
    Arc::new(t)
}

/// R19-7 (`SpawnBehavior.cpp:657-666` then `:727-728`): a spawn through a spawn-point exit is enslaved to the
/// spawner before it leaves, so the unit's `SlavedUpdate` has a master from its first frame.
/// Revert check: drop the master write from `slaved_on_enslave`; the new unit has none (the later write in
/// `spawn_create_spawn` is gone).
#[test]
fn a_spawn_point_spawn_is_enslaved_to_its_spawner() {
    let mut f = ObjectTemplate::new("Site");
    f.max_health = 1000.0;
    f.initial_health = 1000.0;
    f.production = Some(ProductionTemplate {
        exit: ProductionExit::SpawnPoint { bones: vec![SpawnPointBone { pos: (10.0, 0.0, 0.0), angle: 0.0 }] },
        ..Default::default()
    });
    let mut g = GameLogic::new(TerrainLogic::flat(300, 300, 0), 1);
    let site = g.create_object(&Arc::new(f), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let sid = g.spawn_produced_unit(site, &soldier());
    assert_ne!(sid, ObjectId::INVALID, "a spawn was made");
    assert_eq!(g.find_object(sid).and_then(|o| o.master), Some(site), "`m_slaver = slaver->getID()` (`SlavedUpdate.cpp:706`)");
}

/// R19-7 (`ObjectCreationList.cpp:995-1005`): `doStuffToObj`'s `sdu->onEnslave(sourceObj)` makes the OCL's source the
/// new object's master. Revert check: drop the master write from `slaved_on_enslave`.
#[test]
fn an_ocl_created_slave_is_enslaved_to_its_source() {
    let mut g = GameLogic::new(TerrainLogic::flat(300, 300, 0), 1);
    let src = g.create_object(&soldier(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let ocl = Arc::new(OclTemplate {
        name: "SpawnTwo".to_string(),
        nuggets: vec![OclNugget::CreateObject(CreateObjectTemplate {
            is_object: true,
            names_len: 1,
            objects: vec![Some(soldier())],
            count: 1,
            min_health: 1.0,
            max_health: 1.0,
            ..Default::default()
        })],
    });
    g.run_ocl_at(&ocl, src, None, false);
    let made: Vec<ObjectId> = g.objects().filter(|o| o.id != src).map(|o| o.id).collect();
    assert_eq!(made.len(), 1, "one object made");
    assert_eq!(g.find_object(made[0]).and_then(|o| o.master), Some(src), "the OCL source is the master");
}
