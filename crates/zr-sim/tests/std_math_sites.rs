//! Three logic sites that called std's platform math (`f32::hypot`, `f32::cos`/`sin`), against AGENTS.md "Numbers".
//!
//! * `SKIRMISH_TECH_BUILDING_WITHIN_DISTANCE` (`ScriptConditions.cpp:2249-2282`) asks
//!   `getClosestObject(&center, radius, FROM_CENTER_2D)`, whose range test is on squared distances and strict
//!   (`distCalcProc_CenterAndCenter_2D`, `PartitionManager.cpp:788-802`: `abDistSqr < maxDistSqr`). The port tested
//!   `hypot(dx, dy) <= radius`.
//! * The spawner ring (the port's stand-in for an unmodelled exit door, `modules2.rs`) used `angle.cos()`/`.sin()`;
//!   it now uses the sim's `Cos`/`Sin`.

use std::sync::Arc;

use zr_map::scripts::{Action, Condition, OrCondition, ParamValue, Parameter, Script, ScriptList};
use zr_map::{PolygonTrigger, PolygonTriggers};
use zr_sim::modules2::SpawnTemplate;
use zr_sim::template::{kind_of, SimpleTemplateStore};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn sparam(s: &str) -> Parameter {
    Parameter { kind: 0, value: ParamValue::Plain { int: 0, real: 0.0, string: s.to_owned() } }
}
fn rparam(v: f32) -> Parameter {
    Parameter { kind: 1, value: ParamValue::Plain { int: 0, real: v, string: String::new() } }
}
fn iparam(v: i32) -> Parameter {
    Parameter { kind: 0, value: ParamValue::Plain { int: v, real: 0.0, string: String::new() } }
}

/// Player 0 runs one script: tech building within `Zone` + 0 -> `_NEAR`. `Zone` spans (0,0)-(60,80), so
/// `PolygonTrigger::getRadius` is `sqrt(40^2 + 30^2) = 50` (`halfHeight` is `(hi.y + lo.y) / 2`, PolygonTrigger.cpp:292-295)
/// and the centre is (30,40). Player 1 owns a tech building at `at`.
fn tech_building_near(at: Coord3D) -> bool {
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    g.add_player(0, 1000);
    g.add_player(1, 1000);
    let script = Script {
        version: 2,
        name: "near".to_owned(),
        comment: String::new(),
        condition_comment: String::new(),
        action_comment: String::new(),
        active: true,
        one_shot: false,
        easy: true,
        normal: true,
        hard: true,
        subroutine: false,
        delay_seconds: 0,
        or_conditions: vec![OrCondition {
            conditions: vec![Condition {
                version: 4,
                type_id: 0,
                internal_name: Some("SKIRMISH_TECH_BUILDING_WITHIN_DISTANCE".to_owned()),
                params: vec![sparam("x"), rparam(0.0), sparam("Zone")],
            }],
        }],
        actions: vec![Action {
            version: 2,
            type_id: 0,
            internal_name: Some("SET_FLAG".to_owned()),
            params: vec![sparam("_NEAR"), iparam(1)],
        }],
        actions_false: vec![],
    };
    g.scripts_mut().load(vec![(0, ScriptList { scripts: vec![script], groups: Vec::new() })]);
    g.scripts_mut().set_triggers(&PolygonTriggers {
        version: 1,
        triggers: vec![PolygonTrigger {
            name: "Zone".to_owned(),
            layer: String::new(),
            id: 1,
            is_water: false,
            is_river: false,
            river_start: 0,
            points: vec![[0, 0, 0], [60, 0, 0], [60, 80, 0], [0, 80, 0]],
        }],
    });
    let mut t = ObjectTemplate::new("Derrick");
    t.kind_of = kind_of::STRUCTURE | kind_of::TECH_BUILDING;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    let id = g.create_object(&Arc::new(t), at, 0.0, 1);
    assert_eq!(g.find_object(id).expect("created").position, at, "the building stays where it was placed");
    g.update();
    g.scripts().get_flag("_NEAR")
}

/// A building exactly `radius` from the centre (a 30-40-50 triangle, exact in f32) is outside: the engine's test is
/// `abDistSqr < maxDistSqr`. Revert check: with `hypot(..) <= radius` this is true.
#[test]
fn tech_building_exactly_at_the_radius_is_outside() {
    assert!(!tech_building_near(Coord3D::new(60.0, 80.0, 0.0)));
}

/// One unit closer it is inside (guards the test above against a condition that never fires).
#[test]
fn tech_building_inside_the_radius_is_found() {
    assert!(tech_building_near(Coord3D::new(60.0, 79.0, 0.0)));
}

/// A spawner with no modelled exit puts spawn `i` on the 12-unit ring at angle `0.7 * i`, computed with the sim's
/// `Cos`/`Sin` bit for bit. This is a pin, not a revert check: on the machine it was written on, std's `cos`/`sin`
/// give the same bits for these twelve angles, so it passes with the std calls too.
#[test]
fn spawner_ring_uses_the_sim_trig() {
    const N: usize = 12;
    let mut slave = ObjectTemplate::new("Slave");
    slave.kind_of = kind_of::INFANTRY;
    slave.max_health = 100.0;
    slave.initial_health = 100.0;
    let mut slaver = ObjectTemplate::new("Slaver");
    slaver.kind_of = kind_of::STRUCTURE;
    slaver.max_health = 200.0;
    slaver.initial_health = 200.0;
    slaver.spawn = Some(SpawnTemplate {
        spawn_number: N as i32,
        spawn_replace_delay: 0,
        one_shot: false,
        spawn_template_names: vec!["Slave".to_owned()],
        spawned_require_spawner: false,
        initial_burst: N as i32,
        aggregate_health: false,
    });
    let mut g = GameLogic::new(TerrainLogic::flat(80, 80, 0), 1);
    let mut store = SimpleTemplateStore::default();
    store.add(slave);
    g.set_template_source(Arc::new(store));
    let s = g.create_object(&Arc::new(slaver), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    // The batch arrives over several `SPAWN_UPDATE_RATE` periods (SpawnBehavior.cpp:230-249).
    for _ in 0..600 {
        if g.find_object(s).expect("alive").spawned.len() == N {
            break;
        }
        g.update();
    }
    let spawns = g.find_object(s).expect("alive").spawned.clone();
    assert_eq!(spawns.len(), N, "the spawner made its whole batch");
    for (i, id) in spawns.iter().enumerate() {
        let angle = i as f32 * 0.7;
        let p = g.find_object(*id).expect("spawned").position;
        assert_eq!(p.x.to_bits(), (100.0 + zr_sim::math::cos(angle) * 12.0).to_bits(), "spawn {i} x");
        assert_eq!(p.y.to_bits(), (100.0 + zr_sim::math::sin(angle) * 12.0).to_bits(), "spawn {i} y");
    }
}
