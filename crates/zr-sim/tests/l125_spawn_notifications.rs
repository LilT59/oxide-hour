//! Lane 125: `SpawnBehavior`'s slave notifications (`onDamage`/`onDie`).
//!
//! `SpawnBehavior::onDamage` (`SpawnBehavior.cpp:797-821`) tells every spawn that its slaver was hurt, and a spawn with
//! a `SlavedUpdate` runs `onSlaverDamage` -> `AIUpdateInterface::aiGoProne` -> its own `ProneUpdate::goProne`
//! (`SlavedUpdate.cpp:100-106`, `AIUpdate.cpp:3973-3981`, `ProneUpdate.cpp:92-101`). The amount is the damage the
//! *slaver* took. `SpawnBehavior::onDie` (`:134-189`) clears each spawn's slaver (`SlavedUpdate::stopSlavedEffects`,
//! `SlavedUpdate.cpp:94-97`, `:736-744`), invalidates the spawn's producer, and kills every live spawn when
//! `SpawnedRequireSpawner` is set.

use std::sync::Arc;
use zr_sim::body::DamageInfo;
use zr_sim::damage::DamageType;
use zr_sim::die::DieMux;
use zr_sim::modules2::{ProneTemplate, SlavedTemplate, SpawnTemplate};
use zr_sim::template::{kind_of, SimpleTemplateStore};
use zr_sim::{Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

fn slaver(name: &str, require_spawner: bool) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::STRUCTURE;
    t.max_health = 200.0;
    t.initial_health = 200.0;
    t.spawn = Some(SpawnTemplate {
        spawn_number: 2,
        spawn_replace_delay: 0,
        one_shot: false,
        spawn_template_names: vec!["Slave".to_owned()],
        spawned_require_spawner: require_spawner,
        initial_burst: 0,
        aggregate_health: false,
    });
    t.spawn_die_mux = Some(DieMux { death_types: u32::MAX, veterancy_levels: u32::MAX, exempt_status: 0, required_status: 0 });
    t
}

fn slave() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Slave");
    t.kind_of = kind_of::INFANTRY;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    // Every real slave template has an `AIUpdateInterface`: `SlavedUpdate::onSlaverDamage` needs it to reach the
    // spawn's `ProneUpdate` (`SlavedUpdate.cpp:102-105`).
    t.ai_update = Some(0);
    t.slaved = Some(SlavedTemplate {
        guard_max_range: 100,
        guard_wander_range: 0,
        attack_range: 100,
        repair_rate_per_second: 0.0,
        stay_on_same_layer_as_master: false,
    });
    t.prone = Some(ProneTemplate { damage_to_frames_ratio: 1.0 });
    t
}

/// A slaver and the batch of two spawns its first `SpawnBehavior::update` creates (`SpawnBehavior.cpp:192-265`).
fn world(require_spawner: bool) -> (GameLogic, ObjectId, Vec<ObjectId>) {
    let mut g = GameLogic::new(TerrainLogic::flat(80, 80, 0), 1);
    let mut store = SimpleTemplateStore::default();
    store.add(slave());
    g.set_template_source(Arc::new(store));
    let s = g.create_object(&Arc::new(slaver("Slaver", require_spawner)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    // The second entry of the first batch is past one SPAWN_UPDATE_RATE later (SpawnBehavior.cpp:230-235, :249).
    g.run_frames(17);
    let spawns = g.find_object(s).expect("the slaver is alive").spawned.clone();
    assert_eq!(spawns.len(), 2, "the first batch registered its spawns");
    (g, s, spawns)
}

/// Hitting the slaver adds the *slaver's* dealt damage to each slave's prone frames (`onSlaverDamage` ->
/// `ProneUpdate::goProne`). Revert check: without the `onDamage` hook the prone frames stay 0.
#[test]
fn a_hit_on_the_slaver_puts_its_slaves_prone() {
    let (mut g, s, spawns) = world(false);
    g.damage_object(s, DamageInfo { source: ObjectId::INVALID, amount: 12.0, kind: DamageType::SmallArms });
    for id in spawns {
        assert_eq!(g.find_object(id).unwrap().behavior.prone_frames, 12, "the slaver's dealt damage becomes prone frames");
    }
}

/// The slaver's death clears each spawn's slaver and producer and, with `SpawnedRequireSpawner`, kills every live
/// spawn. Revert check: without the `onDie` hook the slaves keep their `master` and stay at full health.
#[test]
fn the_slavers_death_kills_spawns_that_require_a_spawner() {
    let (mut g, s, spawns) = world(true);
    g.damage_object(s, DamageInfo { source: ObjectId::INVALID, amount: 1000.0, kind: DamageType::Unresistable });
    for id in spawns {
        // `Object::kill` takes them through the die pipeline; a synthetic template with no die modules is destroyed by
        // the default `DestroyDie` fallback, so the object is gone.
        assert!(g.find_object(id).is_none(), "the spawn was killed with its slaver");
    }
}

/// Without `SpawnedRequireSpawner` the spawns survive their slaver, but still stop being slaves. Revert check: without
/// the `onDie` hook the slave keeps its `master`.
#[test]
fn spawns_without_require_spawner_survive_their_slaver() {
    let (mut g, s, spawns) = world(false);
    g.damage_object(s, DamageInfo { source: ObjectId::INVALID, amount: 1000.0, kind: DamageType::Unresistable });
    for id in spawns {
        let o = g.find_object(id).expect("alive");
        assert_eq!(o.health(), 100.0, "not killed");
        assert_eq!(o.master, None, "but not slaved any more");
    }
}
