//! Lane 167: `SpawnBehavior`'s replacement logic (`SpawnBehavior.cpp:192-265` update, `:753-782` onSpawnDeath,
//! `:596-750` createSpawn).
//!
//! A spawner keeps a list of frames it owes a spawn at (`m_replacementTimes`). Its first update makes `SpawnNumber`
//! entries (`:203-225`). A spawn's death removes it from the spawner's list and adds `SpawnReplaceDelay + now`
//! (`onSpawnDeath`, `:765-766`, reached from `Object::onDie`, Object.cpp:4657-4665). Every checked update (each
//! `SPAWN_UPDATE_RATE` frames) creates the entries whose frame has passed (`currentTime > replacementTime`, `:249`).

use std::sync::Arc;
use zr_sim::body::DamageInfo;
use zr_sim::damage::DamageType;
use zr_sim::modules2::SpawnTemplate;
use zr_sim::template::{kind_of, SimpleTemplateStore};
use zr_sim::{Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

fn soldier(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::INFANTRY;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t
}

fn spawner(names: &[&str], number: i32, delay: u32, one_shot: bool, aggregate_health: bool) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Spawner");
    t.kind_of = kind_of::STRUCTURE;
    t.max_health = 200.0;
    t.initial_health = 200.0;
    t.spawn = Some(SpawnTemplate {
        spawn_number: number,
        spawn_replace_delay: delay,
        one_shot,
        spawn_template_names: names.iter().map(|n| n.to_string()).collect(),
        spawned_require_spawner: false,
        initial_burst: 0,
        aggregate_health,
    });
    t
}

/// A spawner at the origin, its templates `A` and `B` in the store. The first batch is made by the first update.
fn world(t: ObjectTemplate) -> (GameLogic, ObjectId) {
    let mut g = GameLogic::new(TerrainLogic::flat(80, 80, 0), 1);
    let mut store = SimpleTemplateStore::default();
    store.add(soldier("A"));
    store.add(soldier("B"));
    g.set_template_source(Arc::new(store));
    let s = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    (g, s)
}

/// The spawner's live spawns, in creation order (`m_spawnIDs`).
fn spawns(g: &GameLogic, s: ObjectId) -> Vec<ObjectId> {
    g.find_object(s).map(|o| o.spawned.clone()).unwrap_or_default()
}

/// Kill a spawn through the die pipeline (a template with no die modules is destroyed by the default `DestroyDie`).
fn kill(g: &mut GameLogic, id: ObjectId) {
    g.damage_object(id, DamageInfo { source: ObjectId::INVALID, amount: 1000.0, kind: DamageType::Unresistable });
}

fn names_of(g: &GameLogic, ids: &[ObjectId]) -> Vec<String> {
    ids.iter().filter_map(|id| g.find_object(*id)).map(|o| o.template.name.clone()).collect()
}

/// A dead spawn is owed its replacement `SpawnReplaceDelay` frames after the death, and the next checked update after
/// that makes it (`onSpawnDeath` :765-766, the check :249). Revert checks: without the `onSpawnDeath` notification
/// (`die.rs` `spawn_notify_spawner_of_death`) nothing is owed; without the `replacement_times.push` nothing is made.
#[test]
fn a_dead_spawn_is_replaced_after_the_spawn_replace_delay() {
    let (mut g, s) = world(spawner(&["A"], 1, 30, false, false));
    g.run_frames(3);
    let first = spawns(&g, s);
    assert_eq!(first.len(), 1, "the first batch made its one spawn");
    kill(&mut g, first[0]);
    assert!(spawns(&g, s).is_empty(), "the dead spawn left the spawner's list (onSpawnDeath, :762-768)");
    // Nothing is made while the frame is not past the replacement time (`currentTime > replacementTime`, :249).
    g.run_frames(30);
    assert!(spawns(&g, s).is_empty(), "the replacement waits SpawnReplaceDelay");
    g.run_frames(16);
    assert_eq!(names_of(&g, &spawns(&g, s)), vec!["A".to_string()], "the replacement is made at the next check");
}

/// Two deaths owe two entries; both are past at the next check, so both are made there. The template iterator
/// moves on with each made spawn and wraps (`++m_templateNameIterator`, :647-651): A, B, then A again.
/// Revert check: without `template_index` advancing, the replacements are all `A`.
#[test]
fn replacements_cycle_through_the_template_names() {
    let (mut g, s) = world(spawner(&["A", "B"], 1, 30, false, false));
    g.run_frames(3);
    assert_eq!(names_of(&g, &spawns(&g, s)), vec!["A".to_string()], "the first batch takes the first name");
    let first = spawns(&g, s)[0];
    kill(&mut g, first);
    g.run_frames(50);
    assert_eq!(names_of(&g, &spawns(&g, s)), vec!["B".to_string()], "the iterator moved on to B");
    let second = spawns(&g, s)[0];
    kill(&mut g, second);
    g.run_frames(50);
    assert_eq!(names_of(&g, &spawns(&g, s)), vec!["A".to_string()], "and wraps back to A");
}

/// Two spawns that die together owe two replacements, and the next checked update makes both.
#[test]
fn two_deaths_owe_two_replacements_made_at_the_next_check() {
    let (mut g, s) = world(spawner(&["A"], 2, 30, false, false));
    g.run_frames(17);
    let both = spawns(&g, s);
    assert_eq!(both.len(), 2, "the first batch of two");
    kill(&mut g, both[0]);
    kill(&mut g, both[1]);
    assert!(spawns(&g, s).is_empty());
    g.run_frames(50);
    assert_eq!(spawns(&g, s).len(), 2, "both replacements are made");
}

/// A one-shot spawner stops after its batch (`m_oneShotCountdown <= 0` -> `stopSpawning`, :261-262, :825-831), so a
/// later death owes nothing that is ever made. Revert check: without the stop, the replacement is made.
#[test]
fn a_one_shot_spawner_does_not_replace_a_dead_spawn() {
    let (mut g, s) = world(spawner(&["A"], 1, 0, true, false));
    g.run_frames(3);
    let first = spawns(&g, s);
    assert_eq!(first.len(), 1, "the one-shot batch");
    kill(&mut g, first[0]);
    g.run_frames(60);
    assert!(spawns(&g, s).is_empty(), "stopped for good");
}

/// `AggregateHealth`: a spawner whose last spawn dies is destroyed (`onSpawnDeath`, :772-781). Revert check: without the
/// aggregate branch the spawner survives its last spawn.
#[test]
fn an_aggregate_health_spawner_dies_with_its_last_spawn() {
    let (mut g, s) = world(spawner(&["A"], 1, 0, false, true));
    g.run_frames(3);
    let first = spawns(&g, s);
    assert_eq!(first.len(), 1);
    kill(&mut g, first[0]);
    g.run_frames(1);
    assert!(g.find_object(s).is_none(), "the spawner is destroyed when its spawn count reaches zero");
}

/// Without `AggregateHealth` the spawner outlives its last spawn and keeps owing the replacement.
#[test]
fn a_spawner_without_aggregate_health_survives_its_last_spawn() {
    let (mut g, s) = world(spawner(&["A"], 1, 0, false, false));
    g.run_frames(3);
    let first = spawns(&g, s);
    kill(&mut g, first[0]);
    // The zero-delay entry is past at the next checked update, at most one SPAWN_UPDATE_RATE on (:230-235).
    g.run_frames(17);
    assert!(g.find_object(s).is_some(), "alive");
    assert_eq!(spawns(&g, s).len(), 1, "its zero-delay replacement is made");
}
