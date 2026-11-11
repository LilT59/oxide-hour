//! Lane 172: `PartitionManager`'s cash-value cells and `SKIRMISH_ATTACK_NEAREST_GROUP_WITH_VALUE`.
//!
//! `Object::addValue` (`Object.cpp:4916-4943`) puts an object's build cost into the partition cells its
//! **vision** circle covers, for the owning player only, with a linear falloff from the centre cell
//! (`hLineAddValue`, `PartitionManager.cpp:5809-5834`). `getNearestGroupWithValue` (`:4880`) walks the
//! cells breadth-first from a source and returns the first whose summed value over the allowed players
//! passes `valueRequired`. `ScriptActions::doSkirmishAttackNearestGroupWithValue`
//! (`ScriptActions.cpp:5450-5479`) runs that search over the team's enemies and attack-moves the team's
//! group to the cell.

use std::sync::Arc;

use zr_map::scripts::{Action, Condition, OrCondition, ParamValue, Parameter, Script, ScriptList};
use zr_map::{Dict, DictValue};
use zr_sim::ai_states::{AiStateType, CommandSource};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn gun() -> WeaponTemplate {
    let mut w = WeaponTemplate::new("Gun");
    w.primary_damage = 10.0;
    w.attack_range = 100.0;
    w.min_delay_between_shots = 30;
    w.max_delay_between_shots = 30;
    w
}

/// A vehicle with a locomotor and a weapon, a given build cost and vision range.
fn tank(name: &str, build_cost: i32, vision_range: f32) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 5.0;
    t.geometry.height = 10.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.vision_range = vision_range;
    t.build_cost = build_cost;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(gun()));
    t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    Arc::new(t)
}

fn world() -> GameLogic {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 1000);
    g.add_player(1, 1000);
    g.set_enemies(0, 1);
    g
}

/// `Object::addValue` + `hLineAddValue`: the object's own cell holds its whole build cost, the cells of
/// its vision circle hold a share that falls with the distance, and only the owner's cells are touched.
/// Revert check: leave `update_value_map` out of the frame; every cell reads 0.
#[test]
fn an_objects_build_cost_fills_the_cells_its_vision_covers() {
    let mut g = world();
    g.create_object(&tank("Enemy", 500, 100.0), Coord3D::new(300.0, 100.0, 0.0), 0.0, 1);
    g.update();
    let vm = g.value_map();
    // `worldToCell` floors: (300,100) with a 40-unit cell is cell (7,2).
    assert_eq!(vm.cell_value(7, 2, 1), 500, "the object's cell holds the whole build cost");
    assert_eq!(vm.cell_value(7, 2, 0), 0, "the value lands on the owner's cells only");
    // `cellRadius = ceil(100 / 40) = 3`, `fCellRadius = 4`: the cell 3 out keeps a quarter.
    assert_eq!(vm.cell_value(4, 2, 1), 125, "the far edge of the circle keeps a quarter");
    assert_eq!(vm.cell_value(6, 2, 1), 375, "one cell out keeps three quarters");
    assert_eq!(vm.cell_value(3, 2, 1), 0, "outside the circle");
}

/// `getNearestGroupWithValue` (`PartitionManager.cpp:4880`) + `iterateCellsBreadthFirst` (`:4499`): the
/// nearest cell whose summed enemy value passes the requirement, as the cell's corner
/// (`getCellX() * m_partitionCellSize`). Revert check: return the *farthest* cell (the row-major
/// `getMostValuableLocation` order) — the position is then the object's own cell (280,80), not (160,80).
#[test]
fn the_search_returns_the_nearest_enemy_cell_that_passes_the_value() {
    let mut g = world();
    g.create_object(&tank("Enemy", 500, 100.0), Coord3D::new(300.0, 100.0, 0.0), 0.0, 1);
    g.update();
    let from = Coord3D::new(0.0, 0.0, 0.0);
    let got = g.nearest_group_with_value(0, 100, &from).expect("a cell passes");
    // The value circle is the filled disc of radius 3 cells around (7,2). From cell (0,0) the nearest
    // member is (5,0) — Euclidean distance sqrt(8) = 2.83 < 3 — at Manhattan distance 5; every cell one
    // Manhattan step nearer is outside the disc.
    assert_eq!(got, Coord3D::new(200.0, 0.0, 0.0), "the near edge of the enemy's value circle");
    assert!(g.nearest_group_with_value(0, 600, &from).is_none(), "no cell reaches 600");
    assert!(g.nearest_group_with_value(1, 100, &from).is_none(), "player 1 has no enemies with objects");
}

fn sparam(s: &str) -> Parameter {
    Parameter { kind: 0, value: ParamValue::Plain { int: 0, real: 0.0, string: s.to_owned() } }
}

fn iparam(i: i32) -> Parameter {
    Parameter { kind: 0, value: ParamValue::Plain { int: i, real: 0.0, string: String::new() } }
}

fn action(name: &str, params: Vec<Parameter>) -> Action {
    Action { version: 2, type_id: 0, internal_name: Some(name.to_owned()), params }
}

fn true_condition() -> Condition {
    Condition { version: 4, type_id: 0, internal_name: Some("CONDITION_TRUE".to_owned()), params: Vec::new() }
}

fn one_shot(name: &str, actions: Vec<Action>) -> Script {
    Script {
        version: 2,
        name: name.to_owned(),
        comment: String::new(),
        condition_comment: String::new(),
        action_comment: String::new(),
        active: true,
        one_shot: true,
        easy: true,
        normal: true,
        hard: true,
        subroutine: false,
        delay_seconds: 0,
        or_conditions: vec![OrCondition { conditions: vec![true_condition()] }],
        actions,
        actions_false: Vec::new(),
    }
}

/// Player 0's script team "Alpha" (two tanks around (100, 100)) and player 1's valuable tank at (300, 100).
/// Returns the team's instance key (`TeamFactory::findTeam` mints `alpha#1`, `Team.cpp:393-406`).
fn team_world() -> (GameLogic, String) {
    let mut g = world();
    g.scripts_mut().set_player_name("player0", 0);
    g.scripts_mut().set_player_name("player1", 1);
    let team = Dict(vec![
        ("teamName".to_owned(), DictValue::Ascii("Alpha".to_owned())),
        ("teamOwner".to_owned(), DictValue::Ascii("player0".to_owned())),
        ("teamUnitType1".to_owned(), DictValue::Ascii("Tank".to_owned())),
        ("teamUnitMinCount1".to_owned(), DictValue::Int(2)),
        ("teamUnitMaxCount1".to_owned(), DictValue::Int(2)),
        ("teamReinforcementOrigin".to_owned(), DictValue::Ascii("origin".to_owned())),
        ("teamIsSingleton".to_owned(), DictValue::Bool(false)),
    ]);
    g.scripts_mut().load_teams(&[team]);
    // `TeamFactory::findTeam` (`Team.cpp:393-406`) makes the prototype's first instance.
    let key = g.scripts_mut().find_team("alpha").expect("the team instance exists");
    let t = tank("Tank", 300, 150.0);
    g.create_object_in_team(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, &key);
    g.create_object_in_team(&t, Coord3D::new(100.0, 120.0, 0.0), 0.0, &key);
    g.create_object(&tank("Enemy", 500, 100.0), Coord3D::new(300.0, 100.0, 0.0), 0.0, 1);
    // The partition's value cells are filled at the end of a frame (`ThePartitionManager->UPDATE()`), so
    // the script of the next frame sees them — as it does in the engine, where the script update runs
    // before the partition update.
    g.update();
    (g, key)
}

/// `ScriptActions::doSkirmishAttackNearestGroupWithValue` (`ScriptActions.cpp:5450-5479`): the team's
/// group attack-moves to the nearest enemy value cell, with `CMD_FROM_SCRIPT`. The old stand-in put the
/// team into `AI_HUNT` with no goal. Revert check: restore `self.team_hunt(logic, &team, None)` in the
/// action; the state is `Hunt` and there is no goal position.
#[test]
fn the_skirmish_action_attack_moves_the_team_to_the_nearest_valued_cell() {
    let (mut g, key) = team_world();
    let members: Vec<_> = g.scripts().teams.get(&key).expect("team").members.clone();
    assert_eq!(members.len(), 2, "both tanks joined the team");
    g.scripts_mut().load(vec![(
        0,
        ScriptList {
            scripts: vec![one_shot(
                "attack",
                vec![action("SKIRMISH_ATTACK_NEAREST_GROUP_WITH_VALUE", vec![sparam("Alpha"), iparam(3), iparam(100)])],
            )],
            groups: Vec::new(),
        },
    )]);
    g.update();
    for id in &members {
        let o = g.find_object(*id).expect("alive");
        assert_eq!(o.ai.machine.current, AiStateType::AttackMoveTo, "the group attack-moves");
        assert_eq!(o.ai.machine.goal_position, Coord3D::new(160.0, 80.0, 0.0), "the near edge of the enemy's value circle");
        assert_eq!(o.ai.machine.attack_move_command_source, CommandSource::Script, "CMD_FROM_SCRIPT");
        assert_eq!(o.ai.machine.last_command_source, CommandSource::Script, "the order source is the script");
    }
}
