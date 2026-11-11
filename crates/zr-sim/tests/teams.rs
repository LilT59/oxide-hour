//! Object -> team membership: `Object::setTeam`/`setOrRestoreTeam` (`Object.cpp:851-948`), the
//! prepended `Team::m_TeamMemberList`, default-team assignment at creation (`Object.cpp:312`),
//! `Team::tryToRecruit` (`Team.cpp:2334`), `TEAM_STOP_AND_DISBAND` and `AIUpdateInterface::joinTeam`.
//!
//! Synthetic templates only: no game data.

use std::sync::Arc;

use zr_map::{Dict, DictValue};
use zr_sim::ai_states::AiStateType;
use zr_sim::object::ObjectId;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TemplateSource};

struct Store(Vec<Arc<ObjectTemplate>>);
impl TemplateSource for Store {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

fn rifle() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Rifleman");
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.kind_of = kind_of::INFANTRY;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 100.0, 100.0, 360.0));
    t.vision_range = 100.0;
    Arc::new(t)
}

fn team_dict(name: &str, owner: &str, singleton: bool, priority: i32, attitude: i32, recruitable: bool) -> Dict {
    Dict(vec![
        ("teamName".into(), DictValue::Ascii(name.into())),
        ("teamOwner".into(), DictValue::Ascii(owner.into())),
        ("teamIsSingleton".into(), DictValue::Bool(singleton)),
        ("teamIsAIRecruitable".into(), DictValue::Bool(recruitable)),
        ("teamProductionPriority".into(), DictValue::Int(priority)),
        ("teamAggressiveness".into(), DictValue::Int(attitude)),
    ])
}

/// Two players (`p1` -> 0 neutral, `p2` -> 1) with their default teams `team` and `teamP2`, plus the
/// non-singleton prototype `Raiders` (owned by player 1) that tests recruit into.
fn world() -> GameLogic {
    let mut g = GameLogic::new(zr_sim::TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 10_000);
    g.add_player(1, 10_000);
    g.scripts_mut().load_default_team_protos(&[("team", ""), ("teamP2", "p2")]);
    g.scripts_mut().load_teams(&[
        team_dict("Raiders", "p2", false, 100, 2, true),
        team_dict("Workers", "p2", false, 50, 0, false),
    ]);
    // `Player::addTeamToList` resolves `teamOwner` to a player (`PlayerList::newGame`); the game
    // registers the names in `ScriptEngine::player_names`.
    g.scripts_mut().set_player_name("p2", 1);
    g.init_from_sides(&[("team", 0), ("teamP2", 1)]);
    g.set_template_source(Arc::new(Store(vec![rifle()])));
    g
}

fn make(g: &mut GameLogic, player: u8) -> ObjectId {
    let t = g.find_template("Rifleman").expect("template");
    g.create_object(&t, Coord3D::new(10.0, 10.0, 0.0), 0.0, player)
}

fn members(g: &GameLogic, key: &str) -> Vec<u32> {
    g.scripts().teams.get(key).map(|t| t.members.iter().map(|m| m.0).collect()).unwrap_or_default()
}

/// `Object::Object`'s `setTeam(team)` (`Object.cpp:309-312`) and `prependTo_TeamMemberList`
/// (`Object.cpp:898`): a new object joins its owner's default team at the head of the list.
#[test]
fn a_created_object_joins_its_owners_default_team_at_the_head() {
    let mut g = world();
    let a = make(&mut g, 1);
    let b = make(&mut g, 1);
    assert_eq!(g.find_object(a).unwrap().team.as_deref(), Some("teamp2"));
    assert_eq!(members(&g, "teamp2"), vec![b.0, a.0], "newest member first (prepend)");
    // The neutral player's default team gets a neutral object.
    let n = make(&mut g, 0);
    assert_eq!(g.find_object(n).unwrap().team.as_deref(), Some("team"));
    assert_eq!(members(&g, "team"), vec![n.0]);
}

/// `TheThingFactory->newObject(template, team)`: the object is owned by the team's controlling player
/// and is the first member of that team's list.
#[test]
fn an_object_created_in_a_team_takes_that_teams_owner() {
    let mut g = world();
    let key = g.scripts_mut().create_team("raiders").expect("instance");
    let t = g.find_template("Rifleman").unwrap();
    let id = g.create_object_in_team(&t, Coord3D::new(20.0, 20.0, 0.0), 0.0, &key);
    let o = g.find_object(id).unwrap();
    assert_eq!(o.player, 1, "the team's controlling player");
    assert_eq!(o.team.as_deref(), Some(key.as_str()));
    assert_eq!(members(&g, &key), vec![id.0]);
    assert_eq!(o.ai.attitude, 2, "Object::setTeam takes the prototype's initial attitude");
}

/// `Object::setTeam`/`setOrRestoreTeam` (`Object.cpp:872-918`): leaving the old team's list, joining
/// the new one at its head and taking the new prototype's attitude.
#[test]
fn set_object_team_moves_the_object_between_member_lists() {
    let mut g = world();
    let a = make(&mut g, 1);
    let b = make(&mut g, 1);
    let key = g.scripts_mut().create_team("raiders").unwrap();
    g.set_object_team_key(a, &key);
    assert_eq!(members(&g, "teamp2"), vec![b.0], "left the default team");
    assert_eq!(members(&g, &key), vec![a.0]);
    assert_eq!(g.find_object(a).unwrap().ai.attitude, 2);
    // Back to the default team: it is at the head again (prepend), and the neutral team of a dead
    // player is not used (player 1 is active).
    g.set_object_team_key(a, "teamp2");
    assert_eq!(members(&g, "teamp2"), vec![a.0, b.0]);
    assert_eq!(members(&g, &key), Vec::<u32>::new());
}

/// `~Object` -> `setTeam(nullptr)` (`Object.cpp:623`): a deleted object leaves its team's list.
#[test]
fn a_deleted_object_leaves_its_team() {
    let mut g = world();
    let a = make(&mut g, 1);
    g.destroy_object(a);
    g.run_frames(1);
    assert!(g.find_object(a).is_none());
    assert_eq!(members(&g, "teamp2"), Vec::<u32>::new());
}

/// A list created while the script engine is out of `GameLogic` (a script action firing an OCL, as at
/// AN3 frame 1) still joins the team once the engine is back.
#[test]
fn an_object_created_during_a_script_action_joins_later() {
    let mut g = world();
    let engine = std::mem::take(g.scripts_mut());
    let t = g.find_template("Rifleman").unwrap();
    let id = g.create_object(&t, Coord3D::new(0.0, 0.0, 0.0), 0.0, 1);
    assert_eq!(g.find_object(id).unwrap().team.as_deref(), Some("teamp2"), "the constructor's team key");
    // The member list is joined after the script update (here: the frame's end).
    *g.scripts_mut() = engine;
    g.update();
    assert_eq!(members(&g, "teamp2"), vec![id.0]);
}

/// `Team::tryToRecruit` (`Team.cpp:2334`): reads `obj->getTeam()`. The player's default team is
/// always recruitable; another team only above the recruiting priority and when `IsAIRecruitable`.
#[test]
fn try_to_recruit_reads_the_objects_team() {
    let mut g = world();
    let a = make(&mut g, 1);
    let workers = g.scripts_mut().create_team("workers").unwrap();
    // The AI recruits into a team it is still building (an inactive instance), and `tryToRecruit`
    // only reads *active* source teams (`Team.cpp:2355`), so activate this one first.
    g.scripts_mut().teams.get_mut(&workers).unwrap().created = true;
    g.set_object_team_key(a, &workers);
    // `Workers` has priority 50 and is not AI-recruitable: the default-team member is taken, this one
    // is not.
    let b = make(&mut g, 1);
    let got = zr_sim::script::try_to_recruit(g.scripts(), &g, 1, "Rifleman", (10.0, 10.0), 500.0, 100);
    assert_eq!(got, Some(b), "the default-team member");
    // Above the `Workers` priority the non-recruitable team still counts as not recruitable.
    let got = zr_sim::script::try_to_recruit(g.scripts(), &g, 1, "Rifleman", (10.0, 10.0), 500.0, 200);
    assert_eq!(got, Some(b), "a non-AI-recruitable team is never a source");
    // Make it recruitable and it can be taken.
    g.scripts_mut().teams.get_mut(&workers).unwrap().recruitable = Some(true);
    let got = zr_sim::script::try_to_recruit(g.scripts(), &g, 1, "Rifleman", (10.0, 10.0), 500.0, 200);
    assert!(got == Some(a) || got == Some(b), "both teams are now sources");
}

/// `ScriptActions::doTeamStop(shouldDisband = TRUE)` (`ScriptActions.cpp:4642-4675`): every member is
/// re-idled (`AIGroup::groupIdle` -> `aiIdle(CMD_FROM_SCRIPT)`, `AIGroup.cpp:2082`) and then merged
/// into the controlling player's default team.
#[test]
fn team_stop_and_disband_idles_members_and_returns_them_to_the_default_team() {
    let mut g = world();
    let key = g.scripts_mut().create_team("raiders").unwrap();
    let t = g.find_template("Rifleman").unwrap();
    let a = g.create_object_in_team(&t, Coord3D::new(30.0, 30.0, 0.0), 0.0, &key);
    let b = g.create_object_in_team(&t, Coord3D::new(40.0, 30.0, 0.0), 0.0, &key);
    // Put both into a state that the idle clears (a move in progress).
    g.ai_move_to_position_cmd(a, Coord3D::new(100.0, 100.0, 0.0), zr_sim::ai_states::CommandSource::Ai);
    g.ai_move_to_position_cmd(b, Coord3D::new(100.0, 100.0, 0.0), zr_sim::ai_states::CommandSource::Ai);
    assert_eq!(g.find_object(a).unwrap().ai.machine.current, AiStateType::MoveTo);
    let mut engine = std::mem::take(g.scripts_mut());
    engine.team_stop(&mut g, &key, true);
    *g.scripts_mut() = engine;
    assert_eq!(g.find_object(a).unwrap().ai.machine.current, AiStateType::Idle, "re-idled");
    assert_eq!(g.find_object(b).unwrap().ai.machine.current, AiStateType::Idle, "re-idled");
    assert_eq!(g.find_object(a).unwrap().team.as_deref(), Some("teamp2"));
    let mut expected = vec![a.0, b.0];
    expected.sort_unstable();
    let mut got = members(&g, "teamp2");
    got.sort_unstable();
    assert_eq!(got, expected, "merged into the default team");
    assert_eq!(members(&g, &key), Vec::<u32>::new(), "source team emptied");
}

/// `AIUpdateInterface::joinTeam` (`AIUpdate.cpp:2536-2587`): a reinforcement joins a team mate — an
/// idle mate's position becomes its move goal.
#[test]
fn a_reinforcement_unit_walks_to_its_idle_team_mate() {
    let mut g = world();
    let key = g.scripts_mut().create_team("raiders").unwrap();
    let t = g.find_template("Rifleman").unwrap();
    let mate = g.create_object_in_team(&t, Coord3D::new(60.0, 70.0, 0.0), 0.0, &key);
    let unit = g.create_object_in_team(&t, Coord3D::new(10.0, 10.0, 0.0), 0.0, &key);
    g.ai_idle(unit, zr_sim::ai_states::CommandSource::Ai);
    g.ai_join_team(unit);
    let o = g.find_object(unit).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::MoveTo, "moved to the team mate");
    let goal = o.ai.goal().expect("a move goal");
    // The mate stands on the join point, so the move's `onEnter` adjust (AIStates.cpp:1661-1664) puts the goal on
    // the nearest free cell beside it, not on the mate's centre.
    let mate_pos = g.find_object(mate).unwrap().position;
    assert!(goal.dist_2d(&mate_pos) < 20.0 && goal.dist_2d(&mate_pos) > 1.0, "goal is beside the mate's position: {goal:?}");
}

/// `Team::setActive` + `Team::updateState` (`Team.cpp:1812-1819`): the create script runs on the team
/// pass after activation, not at the `setActive` call itself.
#[test]
fn the_team_create_script_runs_in_the_team_update() {
    let mut g = world();
    let mut d = team_dict("Raiders", "p2", false, 100, 0, true);
    d.0.push(("teamOnCreateScript".into(), DictValue::Ascii("Raider Created".into())));
    g.scripts_mut().load_teams(&[d]);
    let script = zr_map::scripts::Script {
        version: 2,
        name: "Raider Created".into(),
        comment: String::new(),
        condition_comment: String::new(),
        action_comment: String::new(),
        active: true,
        one_shot: false,
        easy: true,
        normal: true,
        hard: true,
        subroutine: true, // only the team's OnCreate run calls it
        delay_seconds: 0,
        or_conditions: vec![zr_map::scripts::OrCondition {
            conditions: vec![zr_map::scripts::Condition {
                version: 4,
                type_id: 0,
                internal_name: Some("CONDITION_TRUE".into()),
                params: Vec::new(),
            }],
        }],
        actions: vec![zr_map::scripts::Action {
            version: 2,
            type_id: 0,
            internal_name: Some("SET_FLAG".into()),
            params: vec![
                zr_map::scripts::Parameter {
                    kind: 5,
                    value: zr_map::scripts::ParamValue::Plain { int: 0, real: 0.0, string: "_CREATED".into() },
                },
                zr_map::scripts::Parameter {
                    kind: 0,
                    value: zr_map::scripts::ParamValue::Plain { int: 1, real: 0.0, string: String::new() },
                },
            ],
        }],
        actions_false: Vec::new(),
    };
    g.scripts_mut().load(vec![(1, zr_map::scripts::ScriptList { scripts: vec![script], groups: Vec::new() })]);
    let key = g.scripts_mut().create_team("raiders").unwrap();
    g.activate_team(&key);
    assert!(!g.scripts().get_flag("_CREATED"), "setActive alone does not run the script");
    g.update();
    assert!(g.scripts().get_flag("_CREATED"), "the team pass ran the create script");
}
