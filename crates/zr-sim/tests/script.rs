//! Synthetic script-engine semantics: flags, counters/timers, OR/AND, one-shot, subroutines,
//! `ENABLE_SCRIPT`/`DISABLE_SCRIPT` and the `EvaluationInterval` gate.

use std::sync::Arc;

use zr_map::scripts::{Action, Condition, OrCondition, ParamValue, Parameter, Script, ScriptGroup, ScriptList};
use zr_map::{Dict, DictValue, PolygonTrigger, PolygonTriggers};
use zr_sim::body::{DamageInfo, DamageType};
use zr_sim::die::{DieMux, DieModuleTemplate, DieTemplate};
use zr_sim::script::comparison;
use zr_sim::script::{ScriptEngine, TeamPrototype};
use zr_sim::template::kind_of;
use zr_sim::{Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

fn iparam(v: i32) -> Parameter {
    Parameter { kind: 0, value: ParamValue::Plain { int: v, real: 0.0, string: String::new() } }
}
fn sparam(s: &str) -> Parameter {
    Parameter { kind: 0, value: ParamValue::Plain { int: 0, real: 0.0, string: s.to_owned() } }
}
fn rparam(v: f32) -> Parameter {
    Parameter { kind: 1, value: ParamValue::Plain { int: 0, real: v, string: String::new() } }
}
/// A string parameter of a specific `Parameter::ParameterType` (COUNTER=4, TEAM=3, FLAG=5, ...).
fn pkind(kind: i32, s: &str) -> Parameter {
    Parameter { kind, value: ParamValue::Plain { int: 0, real: 0.0, string: s.to_owned() } }
}
fn cond(name: &str, params: Vec<Parameter>) -> Condition {
    Condition { version: 4, type_id: 0, internal_name: Some(name.to_owned()), params }
}
fn action(name: &str, params: Vec<Parameter>) -> Action {
    Action { version: 2, type_id: 0, internal_name: Some(name.to_owned()), params }
}
fn script(name: &str, ors: Vec<Vec<Condition>>, actions: Vec<Action>, false_actions: Vec<Action>) -> Script {
    Script {
        version: 2,
        name: name.to_owned(),
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
        or_conditions: ors.into_iter().map(|c| OrCondition { conditions: c }).collect(),
        actions,
        actions_false: false_actions,
    }
}
fn group(name: &str, subroutine: bool, scripts: Vec<Script>) -> ScriptGroup {
    ScriptGroup { version: 2, name: name.to_owned(), active: true, subroutine, scripts }
}

fn logic() -> GameLogic {
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    g.add_player(0, 1000);
    g
}

fn game(scripts: Vec<Script>) -> GameLogic {
    let mut g = logic();
    g.scripts_mut().load(vec![(0, ScriptList { scripts, groups: Vec::new() })]);
    g
}

#[test]
fn set_flag_and_one_shot() {
    let mut g = game(vec![script(
        "flag",
        vec![vec![cond("CONDITION_TRUE", vec![])]],
        vec![action("SET_FLAG", vec![sparam("_TEST"), iparam(1)])],
        vec![],
    )]);
    g.update();
    assert!(g.scripts().get_flag("_TEST"));
}

#[test]
fn or_of_ands() {
    // First OR group is false (flag unset), second is true (a lone TRUE).
    let s = script(
        "or",
        vec![
            vec![cond("FLAG", vec![sparam("_X"), iparam(1)]), cond("CONDITION_TRUE", vec![])],
            vec![cond("CONDITION_TRUE", vec![])],
        ],
        vec![action("SET_FLAG", vec![sparam("_HIT"), iparam(1)])],
        vec![],
    );
    let mut g = game(vec![s]);
    g.update();
    assert!(g.scripts().get_flag("_HIT"));
}

#[test]
fn counters_and_timers() {
    let mut start = script(
        "start",
        vec![vec![cond("CONDITION_TRUE", vec![])]],
        vec![action("SET_MILLISECOND_TIMER", vec![sparam("_T"), rparam(1.0)])],
        vec![],
    );
    start.one_shot = true; // else it would reset the timer every frame
    let expired = script(
        "expired",
        vec![vec![cond("TIMER_EXPIRED", vec![sparam("_T")])]],
        vec![action("SET_FLAG", vec![sparam("_DONE"), iparam(1)])],
        vec![],
    );
    let mut g = game(vec![start, expired]);
    g.update();
    assert!(!g.scripts().get_flag("_DONE"));
    for _ in 0..31 {
        g.update();
    }
    assert!(g.scripts().timer_expired("_T"));
    assert!(g.scripts().get_flag("_DONE"));
}

#[test]
fn subroutine_runs() {
    let sub = group(
        "my sub",
        true,
        vec![script(
            "sub body",
            vec![vec![cond("CONDITION_TRUE", vec![])]],
            vec![action("SET_FLAG", vec![sparam("_SUB"), iparam(1)])],
            vec![],
        )],
    );
    let caller = script(
        "caller",
        vec![vec![cond("CONDITION_TRUE", vec![])]],
        vec![action("CALL_SUBROUTINE", vec![sparam("my sub")])],
        vec![],
    );
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    g.add_player(0, 1000);
    g.scripts_mut().load(vec![(0, ScriptList { scripts: vec![caller], groups: vec![sub] })]);
    g.update();
    assert!(g.scripts().get_flag("_SUB"));
}

#[test]
fn evaluation_interval_gates() {
    let mut s = script(
        "interval",
        vec![vec![cond("CONDITION_TRUE", vec![])]],
        vec![action("INCREMENT_COUNTER", vec![iparam(1), sparam("_N")])],
        vec![],
    );
    s.delay_seconds = 1; // once per 30 frames
    let mut g = game(vec![s]);
    g.update();
    assert_eq!(g.scripts().counter("_N"), 1);
    for _ in 0..29 {
        g.update();
    }
    assert_eq!(g.scripts().counter("_N"), 1, "gated while frame < 30");
    g.update();
    assert_eq!(g.scripts().counter("_N"), 2);
}

#[test]
fn enable_disable_script() {
    let mut target = script(
        "target",
        vec![vec![cond("CONDITION_TRUE", vec![])]],
        vec![action("SET_FLAG", vec![sparam("_RAN"), iparam(1)])],
        vec![],
    );
    target.active = false;
    let toggler = script(
        "toggler",
        vec![vec![cond("CONDITION_TRUE", vec![])]],
        vec![action("ENABLE_SCRIPT", vec![sparam("target")])],
        vec![],
    );
    let mut g = game(vec![target, toggler]);
    g.update(); // target skipped (inactive), toggler enables it
    assert!(!g.scripts().get_flag("_RAN"));
    g.update(); // now target runs
    assert!(g.scripts().get_flag("_RAN"));
}

fn team_dict(name: &str, generic: &[&str]) -> Dict {
    let mut d = Dict(vec![
        ("teamName".into(), DictValue::Ascii(name.into())),
        ("teamOwner".into(), DictValue::Ascii("<This Player>".into())),
        ("teamIsAIRecruitable".into(), DictValue::Bool(true)),
        ("teamProductionPriority".into(), DictValue::Int(100)),
        ("teamProductionPrioritySuccessIncrease".into(), DictValue::Int(20)),
        ("teamProductionPriorityFailureDecrease".into(), DictValue::Int(5)),
    ]);
    for (i, g) in generic.iter().enumerate() {
        d.0.push((format!("teamGenericScriptHook{i}"), DictValue::Ascii((*g).into())));
    }
    d
}

#[test]
fn build_team_runs_generic_script() {
    // A team prototype whose generic script sets a flag; BUILD_TEAM should run it with <This Team>.
    let generic = script(
        "team generic",
        vec![vec![cond("CONDITION_TRUE", vec![])]],
        vec![action("SET_FLAG", vec![sparam("_TEAM_MADE"), iparam(1)])],
        vec![],
    );
    let builder = script(
        "builder",
        vec![vec![cond("CONDITION_TRUE", vec![])]],
        vec![action("BUILD_TEAM", vec![sparam("MyTeam")])],
        vec![],
    );
    let mut g = logic();
    g.scripts_mut().load(vec![(0, ScriptList { scripts: vec![builder, generic], groups: vec![] })]);
    g.scripts_mut().load_teams(&[team_dict("MyTeam", &["team generic"])]);
    let proto: &TeamPrototype = g.scripts().team_proto("myteam").unwrap();
    assert!(proto.is_ai_recruitable);
    assert_eq!(proto.production_priority, 100);
    g.update();
    assert!(g.scripts().get_flag("_TEAM_MADE"), "BUILD_TEAM ran the generic script");
    // The team was empty, so `TeamPrototype::updateState` removed the active instance again.
    assert_eq!(g.scripts().count_team_instances("myteam"), 0);
}

#[test]
fn team_priority_adjust() {
    let mut e = ScriptEngine::default();
    e.load_teams(&[team_dict("MyTeam", &[])]);
    e.set_flag("_x", true);
    e.set_counter("_y", 1);
    e.set_timer_frames("_t", 5);
    assert_eq!(e.team_proto("myteam").unwrap().production_priority, 100);
}

#[test]
fn false_actions_run() {
    let s = script(
        "false branch",
        vec![vec![cond("CONDITION_FALSE", vec![])]],
        vec![action("SET_FLAG", vec![sparam("_TRUE"), iparam(1)])],
        vec![action("SET_FLAG", vec![sparam("_FALSE"), iparam(1)])],
    );
    let mut g = game(vec![s]);
    g.update();
    assert!(!g.scripts().get_flag("_TRUE"));
    assert!(g.scripts().get_flag("_FALSE"));
}

#[test]
fn production_condition_gates_by_difficulty() {
    // `TeamPrototype::evaluateProductionCondition`: the Easy/Normal/Hard variants are gated by the
    // condition script's difficulty flags.
    let mut cond_script = script("cond", vec![vec![cond("CONDITION_TRUE", vec![])]], vec![], vec![]);
    cond_script.easy = true;
    cond_script.normal = false;
    cond_script.hard = false;
    let mut e = ScriptEngine::default();
    e.load(vec![(0, ScriptList { scripts: vec![cond_script], groups: vec![] })]);
    e.set_difficulty(zr_sim::script::Difficulty::Normal);
    let g = logic();
    assert!(!e.evaluate_production_condition(&g, 0, "cond"), "easy-only condition is false at Normal");
    e.set_difficulty(zr_sim::script::Difficulty::Easy);
    assert!(e.evaluate_production_condition(&g, 0, "cond"));
}

#[test]
fn duplicate_and_qualify_is_per_player() {
    // `ScriptList::duplicateAndQualify`: script/group names and SCRIPT/COUNTER/FLAG/TEAM params get
    // the player's qualifier appended, so each player's script state is separate.
    let s = script(
        "my script",
        vec![vec![
            cond("COUNTER", vec![pkind(4, "_ESC"), iparam(3), iparam(1)]),
            cond("TEAM_HAS_UNITS", vec![pkind(3, "Wave Team"), pkind(3, "<This Team>")]),
        ]],
        vec![action("SET_FLAG", vec![pkind(5, "_LAUNCH"), iparam(1)])],
        vec![],
    );
    let list = ScriptList { scripts: vec![s], groups: vec![] };
    let q = list.duplicate_and_qualify("2", "PlyrChina2", "player2");
    let sc = &q.scripts[0];
    assert_eq!(sc.name, "my script2");
    let c = &sc.or_conditions[0].conditions;
    assert_eq!(c[0].params[0].string(), Some("_ESC2"));
    assert_eq!(c[1].params[0].string(), Some("Wave Team2"));
    assert_eq!(c[1].params[1].string(), Some("<This Team>"), "<This Team> is never qualified");
    assert_eq!(sc.actions[0].params[0].string(), Some("_LAUNCH2"));
}

#[test]
fn team_generic_hook0_is_parsed() {
    // The first generic hook is index 0 (`teamGenericScriptHook0`); it was previously skipped.
    let d = team_dict("MyTeam", &["Hook Zero", "Hook One"]);
    let proto = TeamPrototype::from_dict(&d).unwrap();
    assert_eq!(proto.generic_scripts, vec!["Hook Zero".to_owned(), "Hook One".to_owned()]);
}

#[test]
fn set_attack_priority_kind_of_expands_templates() {
    // `ScriptEngine::setPriorityKind`: every template of the given `KindOf` index gets the priority.
    let s = script(
        "prio",
        vec![vec![cond("CONDITION_TRUE", vec![])]],
        vec![action("SET_ATTACK_PRIORITY_KIND_OF", vec![sparam("Power"), iparam(3), iparam(7)])],
        vec![],
    );
    let mut g = game(vec![s]);
    let mut map = std::collections::BTreeMap::new();
    map.insert(3, vec!["Tank".to_owned(), "Truck".to_owned()]);
    g.scripts_mut().set_kind_of_templates(map);
    g.update();
    assert_eq!(g.scripts().attack_priority("Power", "Tank"), 7);
    assert_eq!(g.scripts().attack_priority("Power", "Truck"), 7);
    // `AttackPriorityInfo::getPriority`: an unlisted template has the set's default priority (1).
    assert_eq!(g.scripts().attack_priority("Power", "Plane"), 1);
}

#[test]
fn skirmish_move_to_approach_path_moves_the_team() {
    // `doTeamMoveToSkirmishApproachPath`: move the team to the closest waypoint on the *enemy's*
    // `Center<enemyStart+1>` path.
    let s = script(
        "approach",
        vec![vec![cond("CONDITION_TRUE", vec![])]],
        vec![action("SKIRMISH_MOVE_TO_APPROACH_PATH", vec![pkind(3, "MyTeam"), sparam("Center")])],
        vec![],
    );
    let mut g = approach_world();
    g.scripts_mut().load(vec![(0, ScriptList { scripts: vec![s], groups: Vec::new() })]);
    g.scripts_mut().set_player_meta(0, "China", 0);
    g.scripts_mut().set_player_meta(1, "America", 1);
    g.scripts_mut().set_player_enemy(0, 1);
    let target = Coord3D::new(120.0, 130.0, 0.0);
    g.scripts_mut().set_waypoint_paths(
        vec![
            (1, "wp_near".to_owned(), Coord3D::new(200.0, 200.0, 0.0), vec!["Center1".to_owned()]),
            (2, "wp_enemy".to_owned(), target, vec!["Center2".to_owned()]),
        ],
        std::iter::empty(),
    );
    g.update();
    let unit = g.scripts().teams["myteam"].members[0];
    let o = g.find_object(unit).unwrap();
    // `groupMoveToPosition` is an ordinary move order (`AI_MOVE_TO`), so the pathfinder adjusts the
    // destination to a nearby free cell.
    assert_eq!(o.ai.machine.current, zr_sim::ai_states::AiStateType::MoveTo);
    let end = o.ai.path.last().copied().expect("a path");
    // C++-derived: the single unit (radius 0, centred) is adjusted to the group destination (120,130)
    // (AIGroup.cpp:546). Candidate cell (12,13) gives (125,135) (`adjustCoordToCell`, centred). The zero-length
    // tighten line still runs `iterateCellsAlongLine`'s carry step to (12,14) (AIPathfind.cpp:8387 / iterate),
    // accepted at (125,145); `checkPathCost` is 10 and `1.4*(5+15)` is 28, so it is accepted (AIPathfind.cpp
    // :8409). The approach is 15.8 from the waypoint, so the old `< 15` was port-only.
    assert!(end.dist_2d(&Coord3D::new(125.0, 145.0, 0.0)) < 1.0, "adjusted to the C++ group destination, got {end:?} (waypoint {target:?})");
}

#[test]
fn skirmish_follow_approach_path_follows_the_waypoints_as_a_state() {
    // `doTeamFollowSkirmishApproachPath` -> `groupFollowWaypointPathAsTeam`: every member runs the
    // follow-waypoint-path state (not a bare path), starting at the closest waypoint.
    let s = script(
        "approach",
        vec![vec![cond("CONDITION_TRUE", vec![])]],
        vec![action("SKIRMISH_FOLLOW_APPROACH_PATH", vec![pkind(3, "MyTeam"), sparam("Center"), iparam(1)])],
        vec![],
    );
    let mut g = approach_world();
    g.scripts_mut().load(vec![(0, ScriptList { scripts: vec![s], groups: Vec::new() })]);
    g.scripts_mut().set_player_meta(0, "China", 0);
    g.scripts_mut().set_player_meta(1, "America", 1);
    g.scripts_mut().set_player_enemy(0, 1);
    g.scripts_mut().set_waypoint_paths(
        vec![
            (2, "wp_a".to_owned(), Coord3D::new(120.0, 130.0, 0.0), vec!["Center2".to_owned()]),
            (3, "wp_b".to_owned(), Coord3D::new(60.0, 60.0, 0.0), vec!["Center2".to_owned()]),
        ],
        vec![(2, 3)],
    );
    g.update();
    let unit = g.scripts().teams["myteam"].members[0];
    let o = g.find_object(unit).unwrap();
    assert_eq!(o.ai.machine.current, zr_sim::ai_states::AiStateType::FollowWaypointPath);
    assert!(!o.ai.machine.goal_path.is_empty(), "the linked path from the closest waypoint");
}

#[test]
fn team_garrison_nearest_building_enters_the_building() {
    // `doTeamGarrisonNearestBuilding`: infantry enters the nearest garrisonable building.
    let s = script(
        "garrison",
        vec![vec![cond("CONDITION_TRUE", vec![])]],
        vec![action("TEAM_GARRISON_NEAREST_BUILDING", vec![pkind(3, "MyTeam")])],
        vec![],
    );
    let mut g = garrison_world();
    g.scripts_mut().load(vec![(0, ScriptList { scripts: vec![s], groups: Vec::new() })]);
    g.run_frames(60); // runs the script and lets the infantry walk in
    let infantry = g.find_template("Rifleman").unwrap();
    let id = g.objects().find(|o| o.player == 0 && o.template.name == infantry.name).map(|o| o.id).unwrap();
    assert!(g.is_contained(id), "the infantry garrisoned the bunker");
}

#[test]
fn unit_health_compares_health_percent() {
    // `evaluateUnitHealth`: `(cur*100 + initial/2)/initial` compared with the parameter.
    use std::sync::Arc;
    use zr_sim::body::DamageInfo;
    use zr_sim::damage::DamageType;
    use zr_sim::{ObjectId, ObjectTemplate, TemplateSource};
    struct Store(Arc<ObjectTemplate>);
    impl TemplateSource for Store {
        fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
            (name == self.0.name).then(|| self.0.clone())
        }
    }
    let t = Arc::new(ObjectTemplate::new("Hero"));
    let mut g = logic();
    g.set_template_source(Arc::new(Store(t.clone())));
    let id = g.create_object(&t, Coord3D::new(5.0, 5.0, 0.0), 0.0, 0);
    g.scripts_mut().add_named_object("Hero", id);
    // 80 of 100 health -> 20%.
    g.damage_object(id, DamageInfo { source: ObjectId::INVALID, amount: 80.0, kind: DamageType::Unresistable });
    let s = script(
        "hurt",
        vec![vec![cond("UNIT_HEALTH", vec![sparam("Hero"), iparam(0), iparam(25)])]],
        vec![action("SET_FLAG", vec![sparam("_HURT"), iparam(1)])],
        vec![],
    );
    g.scripts_mut().load(vec![(0, ScriptList { scripts: vec![s], groups: vec![] })]);
    g.update();
    assert!(g.scripts().get_flag("_HURT"), "20% < 25%");
}

#[test]
fn player_sell_everything_sells_structures() {
    use std::sync::Arc;
    use zr_sim::{ObjectTemplate, TemplateSource};
    struct Store(Arc<ObjectTemplate>);
    impl TemplateSource for Store {
        fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
            (name == "Bunker").then(|| self.0.clone())
        }
    }
    let mut b = ObjectTemplate::new("Bunker");
    b.kind_of = kind_of::STRUCTURE;
    b.max_health = 100.0;
    b.initial_health = 100.0;
    b.build_cost = 500;
    let b = Arc::new(b);
    let mut g = logic();
    g.set_template_source(Arc::new(Store(b.clone())));
    let id = g.create_object(&b, Coord3D::new(20.0, 20.0, 0.0), 0.0, 0);
    let s = script(
        "sell",
        vec![vec![cond("CONDITION_TRUE", vec![])]],
        vec![action("PLAYER_SELL_EVERYTHING", vec![sparam("<This Player>")])],
        vec![],
    );
    g.scripts_mut().load(vec![(0, ScriptList { scripts: vec![s], groups: vec![] })]);
    g.update();
    g.run_frames(1);
    assert!(g.find_object(id).is_none(), "the structure was sold");
}

#[test]
fn special_power_events_are_consumed_once() {
    use zr_sim::ObjectId;
    let s = script(
        "sp",
        vec![vec![cond(
            "PLAYER_COMPLETED_SPECIAL_POWER",
            vec![sparam("<This Player>"), sparam("SpecialAbilityRebelCaptureBuilding")],
        )]],
        vec![action("INCREMENT_COUNTER", vec![iparam(1), sparam("_N")])],
        vec![],
    );
    let mut g = logic();
    g.scripts_mut().load(vec![(0, ScriptList { scripts: vec![s], groups: vec![] })]);
    g.scripts_mut()
        .completed_special_powers
        .entry(0)
        .or_default()
        .push(("SpecialAbilityRebelCaptureBuilding".into(), ObjectId::INVALID));
    g.update();
    assert_eq!(g.scripts().counter("_N"), 1);
    g.update();
    assert_eq!(g.scripts().counter("_N"), 1, "the event is removed after the first match");
}

#[test]
fn team_object_status_predicate() {
    use std::sync::Arc;
    use zr_sim::{ObjectTemplate, TemplateSource};
    struct Store(Arc<ObjectTemplate>);
    impl TemplateSource for Store {
        fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
            (name == "Tank").then(|| self.0.clone())
        }
    }
    let mut t = ObjectTemplate::new("Tank");
    t.kind_of = kind_of::CAN_ATTACK;
    let t = Arc::new(t);
    let mut g = logic();
    g.set_template_source(Arc::new(Store(t.clone())));
    let id = g.create_object(&t, Coord3D::new(5.0, 5.0, 0.0), 0.0, 0);
    g.scripts_mut().load_teams(&[team_dict("MyTeam", &[])]);
    let mut team = zr_sim::script::TeamInstance::new("myteam");
    team.created = true;
    team.members.push(id);
    g.scripts_mut().teams.insert("myteam".to_owned(), team);
    let s = script(
        "status",
        vec![vec![cond("TEAM_ALL_HAS_OBJECT_STATUS", vec![pkind(3, "MyTeam"), sparam("CAN_ATTACK")])]],
        vec![action("SET_FLAG", vec![sparam("_OK"), iparam(1)])],
        vec![],
    );
    g.scripts_mut().load(vec![(0, ScriptList { scripts: vec![s], groups: vec![] })]);
    g.update();
    assert!(g.scripts().get_flag("_OK"), "all members have CAN_ATTACK");
}

#[test]
fn victory_action_declares_the_winner() {
    let s = script("win", vec![vec![cond("CONDITION_TRUE", vec![])]], vec![action("VICTORY", vec![])], vec![]);
    let mut g = logic();
    g.add_player(1, 1000);
    g.set_victory_players([0, 1]);
    g.scripts_mut().load(vec![(0, ScriptList { scripts: vec![s], groups: vec![] })]);
    g.update();
    assert_eq!(g.check_for_game_end().winner, Some(0));
}

/// A world with player 0, one infantry unit on team `MyTeam`.
fn approach_world() -> GameLogic {
    use std::sync::Arc;
    use zr_sim::template::{LocomotorKind, LocomotorParams};
    use zr_sim::{ObjectTemplate, TemplateSource};
    struct Store(Vec<Arc<ObjectTemplate>>);
    impl TemplateSource for Store {
        fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
            self.0.iter().find(|t| t.name == name).cloned()
        }
    }
    let mut tank = ObjectTemplate::new("Tank");
    tank.max_health = 100.0;
    tank.initial_health = 100.0;
    tank.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
    tank.vision_range = 100.0;
    let store = Store(vec![Arc::new(tank)]);
    let mut g = logic();
    g.set_template_source(Arc::new(store));
    let id = g.create_object(&g.find_template("Tank").unwrap(), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    g.scripts_mut().load_teams(&[team_dict("MyTeam", &[])]);
    let mut team = zr_sim::script::TeamInstance::new("myteam");
    team.created = true;
    team.members.push(id);
    g.scripts_mut().teams.insert("myteam".to_owned(), team);
    g
}

/// A world with player 0, one infantry unit and a garrison bunker.
fn garrison_world() -> GameLogic {
    use std::sync::Arc;
    use zr_sim::contain::{ContainKind, ContainTemplate};
    use zr_sim::template::{LocomotorKind, LocomotorParams};
    use zr_sim::{ObjectTemplate, TemplateSource};
    struct Store(Vec<Arc<ObjectTemplate>>);
    impl TemplateSource for Store {
        fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
            self.0.iter().find(|t| t.name == name).cloned()
        }
    }
    let mut rifle = ObjectTemplate::new("Rifleman");
    rifle.max_health = 100.0;
    rifle.initial_health = 100.0;
    rifle.kind_of = kind_of::INFANTRY;
    rifle.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 100.0, 100.0, 360.0));
    let mut bunker = ObjectTemplate::new("Bunker");
    bunker.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    bunker.max_health = 500.0;
    bunker.initial_health = 500.0;
    bunker.contain = Some(Arc::new(ContainTemplate {
        kind: ContainKind::Garrison,
        capacity: 4,
        allow_inside_kind_of: Some(kind_of::INFANTRY),
        forbid_inside_kind_of: 0,
        allow_allies_inside: true,
        allow_enemies_inside: false,
        allow_neutral_inside: false,
        passengers_allowed_to_fire: true,
        damage_percentage_to_units: 0.0,
        number_of_exit_paths: 1,
        exit_delay: 0,
        door_open_time: 0,
        scatter_nearby_on_exit: false,
        orient_like_container_on_exit: false,
        reset_mood_check_on_exit: false,
        health_regen: 0.0,
        do_i_heal_objects: false,
        heal_frames: 0.0,
        initial_payload: Vec::new(),
        tunnel_frames_for_full_heal: 0.0,
        is_enclosing_container: false,
        fire_points: Default::default(),
        passengers_in_turret: false,
        weapon_bonus_passed_to_passengers: false,
        immune_to_clear_building_attacks: false,
    }));
    let store = Store(vec![Arc::new(rifle), Arc::new(bunker)]);
    let mut g = logic();
    g.set_template_source(Arc::new(store));
    let unit = g.create_object(&g.find_template("Rifleman").unwrap(), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    g.create_object(&g.find_template("Bunker").unwrap(), Coord3D::new(16.0, 10.0, 0.0), 0.0, 0);
    g.scripts_mut().load_teams(&[team_dict("MyTeam", &[])]);
    let mut team = zr_sim::script::TeamInstance::new("myteam");
    team.created = true;
    team.members.push(unit);
    g.scripts_mut().teams.insert("myteam".to_owned(), team);
    g
}

/// `evaluateAndProgressAllSequentialScripts`: one action per step, a `TEAM_SPIN_FOR_FRAMECOUNT`
/// wait, then the next actions (the not-contained wait passes at once for an uncontained team).
#[test]
fn sequential_script_runs_actions_in_order_with_waits() {
    let mut seq = script(
        "seq",
        vec![],
        vec![
            action("SET_FLAG", vec![sparam("_STEP1"), iparam(1)]),
            action("TEAM_SPIN_FOR_FRAMECOUNT", vec![pkind(3, "MyTeam"), iparam(10)]),
            action("SET_FLAG", vec![sparam("_STEP2"), iparam(1)]),
            action("TEAM_WAIT_FOR_NOT_CONTAINED_ALL", vec![pkind(3, "MyTeam")]),
            action("SET_FLAG", vec![sparam("_STEP3"), iparam(1)]),
        ],
        vec![],
    );
    seq.subroutine = true;
    let mut start = script(
        "start",
        vec![vec![cond("CONDITION_TRUE", vec![])]],
        vec![action("TEAM_EXECUTE_SEQUENTIAL_SCRIPT", vec![pkind(3, "MyTeam"), sparam("seq")])],
        vec![],
    );
    start.one_shot = true;
    let mut g = approach_world();
    g.scripts_mut().load(vec![(0, ScriptList { scripts: vec![start, seq], groups: Vec::new() })]);
    g.update();
    assert!(g.scripts().get_flag("_STEP1"), "the first action runs on the frame the script starts");
    assert!(!g.scripts().get_flag("_STEP2"), "the framecount wait holds back the next action");
    for _ in 0..6 {
        g.update();
    }
    assert!(!g.scripts().get_flag("_STEP2"), "still waiting");
    for _ in 0..10 {
        g.update();
    }
    assert!(g.scripts().get_flag("_STEP2") && g.scripts().get_flag("_STEP3"), "waits released, remaining actions ran");
}

/// `TEAM_EXECUTE_SEQUENTIAL_SCRIPT_LOOPING` with count 2 runs the script twice; a moving team holds
/// the script until it is idle again (the approach-path action sets the unit's path).
#[test]
fn sequential_script_loops_and_waits_for_idle() {
    let mut seq = script("seq", vec![], vec![action("INCREMENT_COUNTER", vec![iparam(1), pkind(4, "_C")])], vec![]);
    seq.subroutine = true;
    let mut start = script(
        "start",
        vec![vec![cond("CONDITION_TRUE", vec![])]],
        vec![action("TEAM_EXECUTE_SEQUENTIAL_SCRIPT_LOOPING", vec![pkind(3, "MyTeam"), sparam("seq"), iparam(2)])],
        vec![],
    );
    start.one_shot = true;
    let mut g = approach_world();
    g.scripts_mut().load(vec![(0, ScriptList { scripts: vec![start, seq], groups: Vec::new() })]);
    g.run_frames(20);
    assert_eq!(g.scripts().counter("_C"), 2);
}

/// Retail `Team::updateState`: `OnIdle` fires every frame once the team has been idle two frames
/// running, regardless of `teamInitialIdleFrames`.
#[test]
fn team_on_idle_fires_every_idle_frame() {
    let mut idle = script(
        "on idle",
        vec![vec![cond("CONDITION_TRUE", vec![])]],
        vec![action("INCREMENT_COUNTER", vec![iparam(1), pkind(4, "_IDLE")])],
        vec![],
    );
    idle.subroutine = true;
    let mut g = approach_world();
    let mut d = team_dict("MyTeam", &[]);
    d.0.push(("teamOnIdleScript".into(), DictValue::Ascii("on idle".into())));
    d.0.push(("teamInitialIdleFrames".into(), DictValue::Int(300)));
    g.scripts_mut().load_teams(&[d]);
    g.scripts_mut().load(vec![(0, ScriptList { scripts: vec![idle], groups: Vec::new() })]);
    g.run_frames(10);
    let n = g.scripts().counter("_IDLE");
    assert!((7..=9).contains(&n), "fired on frames 2.. of 10, got {n}");
}

/// Retail `Team::updateState` enemy-sighted scan: an enemy inside a member's vision range fires
/// `OnEnemySighted` once, losing it fires `OnAllClear` once (no attack order involved).
#[test]
fn team_enemy_sighted_uses_the_vision_scan() {
    let mut seen = script("seen", vec![vec![cond("CONDITION_TRUE", vec![])]], vec![action("INCREMENT_COUNTER", vec![iparam(1), pkind(4, "_SEEN")])], vec![]);
    seen.subroutine = true;
    let mut clear = script("clear", vec![vec![cond("CONDITION_TRUE", vec![])]], vec![action("INCREMENT_COUNTER", vec![iparam(1), pkind(4, "_CLEAR")])], vec![]);
    clear.subroutine = true;
    let mut g = approach_world();
    g.add_player(1, 0);
    g.set_relationship_symmetric(0, 1, zr_sim::damage::Relationship::Enemies);
    let mut d = team_dict("MyTeam", &[]);
    d.0.push(("teamEnemySightedScript".into(), DictValue::Ascii("seen".into())));
    d.0.push(("teamAllClearScript".into(), DictValue::Ascii("clear".into())));
    g.scripts_mut().load_teams(&[d]);
    g.scripts_mut().load(vec![(0, ScriptList { scripts: vec![seen, clear], groups: Vec::new() })]);
    g.run_frames(3);
    assert_eq!(g.scripts().counter("_SEEN"), 0, "no enemy around");
    let tank = g.find_template("Tank").unwrap();
    // Out of the 100-unit vision range: still nothing.
    let far = g.create_object(&tank, Coord3D::new(150.0, 10.0, 0.0), 0.0, 1);
    g.run_frames(3);
    assert_eq!(g.scripts().counter("_SEEN"), 0, "enemy outside vision range");
    g.destroy_object(far);
    let near = g.create_object(&tank, Coord3D::new(60.0, 10.0, 0.0), 0.0, 1);
    g.run_frames(5);
    assert_eq!(g.scripts().counter("_SEEN"), 1);
    assert_eq!(g.scripts().counter("_CLEAR"), 0);
    g.destroy_object(near);
    g.run_frames(5);
    assert_eq!(g.scripts().counter("_SEEN"), 1, "fires once per sighting");
    assert_eq!(g.scripts().counter("_CLEAR"), 1);
}

/// `doMergeTeamIntoTeam` + `Team::~Team` -> `notifyOfTeamDestruction`: the emptied source team is
/// removed on the next team update and its sequential scripts go with it.
#[test]
fn team_merge_moves_members_and_drops_source_sequential_scripts() {
    let mut seq = script(
        "seq",
        vec![],
        vec![
            action("TEAM_SPIN_FOR_FRAMECOUNT", vec![pkind(3, "MyTeam"), iparam(5)]),
            action("SET_FLAG", vec![sparam("_AFTER"), iparam(1)]),
        ],
        vec![],
    );
    seq.subroutine = true;
    let mut start = script(
        "start",
        vec![vec![cond("CONDITION_TRUE", vec![])]],
        vec![action("TEAM_EXECUTE_SEQUENTIAL_SCRIPT", vec![pkind(3, "MyTeam"), sparam("seq")])],
        vec![],
    );
    start.one_shot = true;
    let mut merge = script(
        "merge",
        vec![vec![cond("CONDITION_TRUE", vec![])]],
        vec![action("TEAM_MERGE_INTO_TEAM", vec![pkind(3, "MyTeam"), pkind(3, "Other")])],
        vec![],
    );
    merge.one_shot = true;
    let mut g = approach_world();
    g.scripts_mut().load_teams(&[team_dict("MyTeam", &[]), team_dict("Other", &[])]);
    let mut other = zr_sim::script::TeamInstance::new("other");
    other.created = true;
    g.scripts_mut().teams.insert("other".to_owned(), other);
    g.scripts_mut().load(vec![(0, ScriptList { scripts: vec![start, seq, merge], groups: Vec::new() })]);
    g.run_frames(30);
    assert!(!g.scripts().teams.contains_key("myteam"),"emptied source team is deleted");
    assert_eq!(g.scripts().teams.get("other").map(|t| t.members.len()), Some(1));
    assert!(!g.scripts().get_flag("_AFTER"), "the source team's sequential script is gone");
}

/// Templates for the team-exit tests: a rifleman, a ground transport and an airborne transport.
fn exit_world() -> GameLogic {
    use std::sync::Arc;
    use zr_sim::contain::{ContainKind, ContainTemplate, GarrisonFirePoints};
    use zr_sim::template::{LocomotorKind, LocomotorParams};
    use zr_sim::{ObjectTemplate, TemplateSource};
    struct Store(Vec<Arc<ObjectTemplate>>);
    impl TemplateSource for Store {
        fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
            self.0.iter().find(|t| t.name == name).cloned()
        }
    }
    let contain = || ContainTemplate {
        kind: ContainKind::Transport,
        capacity: 2,
        allow_inside_kind_of: Some(kind_of::INFANTRY),
        forbid_inside_kind_of: 0,
        allow_allies_inside: true,
        allow_enemies_inside: true,
        allow_neutral_inside: true,
        passengers_allowed_to_fire: false,
        damage_percentage_to_units: 0.0,
        number_of_exit_paths: 1,
        exit_delay: 0,
        door_open_time: 1,
        scatter_nearby_on_exit: true,
        orient_like_container_on_exit: false,
        reset_mood_check_on_exit: false,
        health_regen: 0.0,
        do_i_heal_objects: false,
        heal_frames: 0.0,
        initial_payload: Vec::new(),
        tunnel_frames_for_full_heal: 1.0,
        is_enclosing_container: true,
        fire_points: GarrisonFirePoints::default(),
        passengers_in_turret: false,
        weapon_bonus_passed_to_passengers: false,
        immune_to_clear_building_attacks: false,
    };
    let mut rifle = ObjectTemplate::new("Rifleman");
    rifle.max_health = 100.0;
    rifle.initial_health = 100.0;
    rifle.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    rifle.transport_slot_count = 1;
    rifle.geometry.major_radius = 4.0;
    rifle.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 100.0, 100.0, 360.0));
    let mut truck = ObjectTemplate::new("Truck");
    truck.kind_of = kind_of::VEHICLE;
    truck.max_health = 200.0;
    truck.initial_health = 200.0;
    truck.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 100.0, 100.0, 120.0));
    truck.contain = Some(Arc::new(contain()));
    let mut plane = ObjectTemplate::new("Chinook");
    plane.kind_of = kind_of::AIRCRAFT;
    plane.max_health = 200.0;
    plane.initial_health = 200.0;
    let mut air = LocomotorParams::from_ini(LocomotorKind::Thrust, 60.0, 100.0, 100.0, 360.0);
    // `AirborneTargetingHeight = 30` (the retail Chinook's value). Not glued to the ground: `StickToGround` defaults to
    // No (Locomotor.cpp:334), and the port's `from_ini` base defaults it to Yes, which landed this plane in one frame.
    // `NoZMotiveForce`: the fixture has no physics module (no gravity, no lift), so the plane keeps the height it is
    // placed at. (The retail Chinook is `SURFACE_RELATIVE_HEIGHT` with physics lift; that would pull a plane placed
    // on the ground back up to its preferred height as soon as the AI runs.)
    air.airborne_targeting_height = 30.0;
    air.stick_to_ground = false;
    plane.locomotor = Some(air);
    plane.contain = Some(Arc::new(contain()));
    let mut g = logic();
    g.set_template_source(Arc::new(Store(vec![Arc::new(rifle), Arc::new(truck), Arc::new(plane)])));
    g
}

/// Puts `members` on the script team `MyTeam` and runs one `TEAM_EXIT_ALL` on it.
fn team_exit_all(g: &mut GameLogic, members: &[zr_sim::ObjectId]) {
    g.scripts_mut().load_teams(&[team_dict("MyTeam", &[])]);
    let mut team = zr_sim::script::TeamInstance::new("myteam");
    team.created = true;
    team.members.extend_from_slice(members);
    g.scripts_mut().teams.insert("myteam".to_owned(), team);
    let mut s = script(
        "exit",
        vec![vec![cond("CONDITION_TRUE", vec![])]],
        vec![action("TEAM_EXIT_ALL", vec![pkind(3, "MyTeam")])],
        vec![],
    );
    s.one_shot = true;
    g.scripts_mut().load(vec![(0, ScriptList { scripts: vec![s], groups: Vec::new() })]);
}

/// Boards `rider` into `container` and waits for it to arrive.
fn board(g: &mut GameLogic, rider: zr_sim::ObjectId, container: zr_sim::ObjectId) {
    g.submit(0, zr_sim::Command::Enter { ids: vec![rider], container });
    g.run_frames(120);
    assert!(g.is_contained(rider), "the rider boarded");
}

#[test]
fn team_exit_all_empties_a_member_ground_transport() {
    // `AIGroup::groupEvacuate` (`AIGroup.cpp:2475`): a ground member orders its passengers out
    // (`aiEvacuate`, `AIUpdate.cpp:3922`).
    let mut g = exit_world();
    let truck_t = g.find_template("Truck").unwrap();
    let rifle_t = g.find_template("Rifleman").unwrap();
    let truck = g.create_object(&truck_t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let rider = g.create_object(&rifle_t, Coord3D::new(110.0, 100.0, 0.0), 0.0, 0);
    board(&mut g, rider, truck);
    team_exit_all(&mut g, &[truck, rider]);
    g.run_frames(60);
    assert!(!g.is_contained(rider), "the member transport evacuated its rider");
}

#[test]
fn team_exit_all_leaves_a_rider_whose_transport_is_not_a_member() {
    // `groupEvacuate` only visits team members. A rider has no container of its own to empty, so
    // with the transport outside the team it stays aboard.
    let mut g = exit_world();
    let truck_t = g.find_template("Truck").unwrap();
    let rifle_t = g.find_template("Rifleman").unwrap();
    let truck = g.create_object(&truck_t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let rider = g.create_object(&rifle_t, Coord3D::new(110.0, 100.0, 0.0), 0.0, 0);
    board(&mut g, rider, truck);
    team_exit_all(&mut g, &[rider]);
    g.run_frames(60);
    assert!(g.is_contained(rider), "the transport is not on the team, so nothing evacuates it");
}

/// `groupEvacuate` (AIGroup.cpp:2483-2488): an airborne aircraft member runs `aiMoveToAndEvacuate` at the ground below it
/// instead of ordering its passengers out at once. The fixture plane holds its altitude (`exit_world`), so the
/// `OBJECT_STATUS_AIRBORNE_TARGET` bit (`doLocomotor`, AIUpdate.cpp:2294-2300) is set.
#[test]
fn team_exit_all_sends_an_airborne_member_to_evacuate_below_itself() {
    let mut g = exit_world();
    let plane_t = g.find_template("Chinook").unwrap();
    let rifle_t = g.find_template("Rifleman").unwrap();
    let plane = g.create_object(&plane_t, Coord3D::new(100.0, 100.0, 60.0), 0.0, 0);
    let rider = g.create_object(&rifle_t, Coord3D::new(110.0, 100.0, 0.0), 0.0, 0);
    board(&mut g, rider, plane);
    team_exit_all(&mut g, &[plane, rider]);
    g.run_frames(60);
    let goal = g.find_object(plane).unwrap().ai.machine.goal_position;
    assert!((goal.z - 0.0).abs() < 0.01 && (goal.x - 100.0).abs() < 0.01, "the drop point is on the ground below the plane, got {goal:?}");
    assert!(!g.is_contained(rider), "the airborne transport evacuated at its drop point");
}

/// `groupEvacuate` (AIGroup.cpp:2483): only a plane with the `OBJECT_STATUS_AIRBORNE_TARGET` bit moves to the ground below
/// itself. A plane on the ground (height 0, not above `AirborneTargetingHeight`) has no bit, so it orders its passengers
/// out where it is, though its locomotor is an air one (`isAirborne` is the kind, not the height).
#[test]
fn team_exit_all_orders_an_aircraft_without_the_airborne_bit_out_in_place() {
    let mut g = exit_world();
    let plane_t = g.find_template("Chinook").unwrap();
    let rifle_t = g.find_template("Rifleman").unwrap();
    let plane = g.create_object(&plane_t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let rider = g.create_object(&rifle_t, Coord3D::new(110.0, 100.0, 0.0), 0.0, 0);
    board(&mut g, rider, plane);
    team_exit_all(&mut g, &[plane, rider]);
    g.run_frames(60);
    let goal = g.find_object(plane).unwrap().ai.machine.goal_position;
    assert!(!(goal.z.abs() < 0.01 && (goal.x - 100.0).abs() < 0.01), "no move to the drop point is ordered, got {goal:?}");
}

/// `ActiveBody::internalChangeHealth` reflags `EFFECTIVELY_DEAD` whenever health is `<= 0`
/// (`ActiveBody.cpp:1255`), so `Object::isEffectivelyDead()` is true for a 0-health object even when
/// its die modules keep it in the world (`KeepObjectDie`). The script readers therefore use the port's
/// combined `Object::is_dying()` (`effectively_dead || is_dead()`), not the `effectively_dead` bit
/// alone: `NAMED_*` (`ScriptConditions.cpp:306-330`), `count_owned` (`Team.cpp:1628`) and
/// `count_owned_in_area` (`ScriptConditions.cpp:494-497`).
#[test]
fn zero_health_objects_are_effectively_dead_for_scripts() {
    let mut g = game(vec![
        script(
            "named alive",
            vec![vec![cond("NAMED_NOT_DESTROYED", vec![sparam("Doomed")])]],
            vec![action("SET_FLAG", vec![sparam("_ALIVE"), iparam(1)])],
            vec![],
        ),
        script(
            "named dead",
            vec![vec![cond("NAMED_DESTROYED", vec![sparam("Doomed")])]],
            vec![action("SET_FLAG", vec![sparam("_DEAD"), iparam(1)])],
            vec![],
        ),
        script(
            "count",
            vec![vec![cond(
                "PLAYER_HAS_OBJECT_COMPARISON",
                vec![sparam("x"), iparam(comparison::EQUAL), iparam(1), sparam("Victim")],
            )]],
            vec![action("SET_FLAG", vec![sparam("_COUNTED"), iparam(1)])],
            vec![],
        ),
        script(
            "area count",
            vec![vec![cond(
                "PLAYER_HAS_COMPARISON_UNIT_TYPE_IN_TRIGGER_AREA",
                vec![sparam("x"), iparam(comparison::EQUAL), iparam(1), sparam("Victim"), sparam("Zone")],
            )]],
            vec![action("SET_FLAG", vec![sparam("_AREA_COUNTED"), iparam(1)])],
            vec![],
        ),
    ]);
    g.scripts_mut().set_triggers(&PolygonTriggers {
        version: 1,
        triggers: vec![PolygonTrigger {
            name: "Zone".to_owned(),
            layer: String::new(),
            id: 1,
            is_water: false,
            is_river: false,
            river_start: 0,
            points: vec![[0, 0, 0], [100, 0, 0], [100, 100, 0], [0, 100, 0]],
        }],
    });
    let mut t = ObjectTemplate::new("Victim");
    t.max_health = 100.0;
    t.initial_health = 100.0;
    // `FXListDie` without an FX does nothing to the object: it stays in the world at 0 health, the
    // same state `KeepObjectDie` leaves (the engine still flags it `EFFECTIVELY_DEAD`).
    t.die = vec![DieModuleTemplate {
        mux: DieMux { death_types: u32::MAX, veterancy_levels: u32::MAX, exempt_status: 0, required_status: 0 },
        kind: DieTemplate::FxList { default_death_fx: None, initially_active: false, orient_to_object: false },
    }];
    let id = g.create_object(&Arc::new(t), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    g.scripts_mut().add_named_object("Doomed", id);
    // No die modules: the object stays in the world at 0 health (the `KeepObjectDie` case), so the
    // port's `effectively_dead` bit stays clear until the die pipeline runs.
    g.damage_object(id, DamageInfo { source: ObjectId::INVALID, amount: 200.0, kind: DamageType::Unresistable });
    let o = g.find_object(id).expect("kept in the world");
    assert_eq!(o.health(), 0.0);
    assert!(!o.is_destroyed(), "no die module destroyed it");
    assert!(o.is_dying(), "health 0 is isEffectivelyDead in the engine");
    g.update();
    assert!(!g.scripts().get_flag("_ALIVE"), "a 0-health object is not alive");
    assert!(g.scripts().get_flag("_DEAD"), "NAMED_DESTROYED reports it");
    assert!(!g.scripts().get_flag("_COUNTED"), "count_owned skips it");
    assert!(!g.scripts().get_flag("_AREA_COUNTED"), "count_owned_in_area skips it");
}
