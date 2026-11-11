//! Lane 173 (wave 20): R19-6 — `SpawnBehavior::computeAggregateStates` (`SpawnBehavior.cpp:862-1020`) through the
//! sim, on synthetic data only.

use std::sync::Arc;
use zr_sim::template::{kind_of, ObjectTemplate, SimpleTemplateStore};
use zr_sim::{Coord3D, GameLogic, ObjectId, TerrainLogic};

fn unit(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t
}

fn health(g: &GameLogic, id: ObjectId) -> f32 {
    g.find_object(id).and_then(|o| o.body.as_ref()).map_or(-1.0, |b| b.health())
}

/// A spawner with `AggregateHealth` and two spawns. Revert check: drop the `spawn_compute_aggregate_states` call from
/// `spawn_update`; the spawner keeps its own 100 health after a spawn is damaged.
#[test]
fn an_aggregate_health_spawner_takes_its_spawns_health() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    g.add_player(0, 0);
    let mut store = SimpleTemplateStore::default();
    let spawned = store.add(unit("Spawned"));
    g.set_template_source(Arc::new(store));
    let mut t = unit("Spawner");
    t.kind_of = kind_of::STRUCTURE;
    t.spawn = Some(zr_sim::modules2::SpawnTemplate {
        spawn_number: 2,
        spawn_replace_delay: 9999,
        one_shot: false,
        spawn_template_names: vec![spawned.name.clone()],
        spawned_require_spawner: false,
        initial_burst: 0,
        aggregate_health: true,
    });
    let id: ObjectId = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    // Two checks: the first entry is due at once, the second at the next 15-frame check (`SpawnBehavior.cpp:244-259`).
    g.run_frames(20);
    let spawns: Vec<ObjectId> = g.objects().filter(|o| o.template.name == "Spawned").map(|o| o.id).collect();
    assert_eq!(spawns.len(), 2, "the spawner made its batch");
    // Full health: `sum(100+100) / (100 * 2) = 1` -> `setInitialHealth(100)` -> the 100 it already had.
    assert_eq!(health(&g, id), 100.0, "two healthy spawns are the whole bar");
    // Damage one spawn down to half: `sum(50+100)/(100*2) = 0.75` -> `setInitialHealth(75)` -> 75 of its 100 initial.
    g.damage_object_with_death(
        spawns[0],
        zr_sim::body::DamageInfo { source: ObjectId::INVALID, amount: 50.0, kind: zr_sim::damage::DamageType::Unresistable },
        zr_sim::damage::DeathType::Normal,
    );
    assert_eq!(health(&g, spawns[0]), 50.0, "the spawn is at half health");
    g.run_frames(1);
    assert_eq!(health(&g, id), 75.0, "the spawner's bar is `100 * sum(health) / (avgMax * SpawnNumber)`");
}
