//! Lane 151: objects a script action creates must belong to their team at once, exactly as
//! `Object::Object` -> `setTeam` (`Object.cpp:312`, `:898`) does in the original, where the engine
//! is a singleton the whole time.
//!
//! `GameLogic::script_engine_update` takes the `ScriptEngine` out of `GameLogic` while scripts run,
//! so a script action that reads `logic.scripts` sees an empty engine: no teams, no owners, no
//! waypoints. The action's own engine decides the new object's owner and member-list insert
//! (`create_object_in_team_with`), because the rest of the same action iterates the team again
//! (`ScriptActions.cpp:607`, `:662`, `:730`).

use std::sync::Arc;

use zr_map::scripts::{Action, Condition, OrCondition, ParamValue, Parameter, Script, ScriptList};
use zr_map::{Dict, DictValue};
use zr_sim::body::MaxHealthChangeType;
use zr_sim::template::kind_of;
use zr_sim::upgrade::{SimUpgradeModule, UpgradeEffect};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TemplateSource, TerrainLogic};

fn sparam(s: &str) -> Parameter {
    Parameter { kind: 0, value: ParamValue::Plain { int: 0, real: 0.0, string: s.to_owned() } }
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

struct Store(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for Store {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

/// `Upgrade_Big` is a player upgrade that adds 100 max health (an `UpgradeModule`'s effect, the way
/// an armour/weapon-set upgrade reaches a new object). Player 0 runs the scripts; player 1 owns the
/// team, so only a creation that resolves the owner from the team sees the upgrade.
fn world() -> GameLogic {
    let mut tank = ObjectTemplate::new("Tank");
    tank.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    tank.geometry.major_radius = 5.0;
    tank.max_health = 100.0;
    tank.initial_health = 100.0;
    tank.locomotor = Some(zr_sim::LocomotorParams::from_ini(zr_sim::LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    tank.upgrade_modules = vec![Arc::new(SimUpgradeModule {
        triggered_by: vec!["Upgrade_Big".to_owned()],
        conflicts_with: Vec::new(),
        removes_upgrades: Vec::new(),
        requires_all_triggers: false,
        effect: UpgradeEffect::MaxHealth { add_max_health: 100.0, change_type: MaxHealthChangeType::SameCurrentHealth },
    })];
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 1000);
    g.add_player(1, 1000);
    g.set_template_source(Arc::new(Store(vec![Arc::new(tank)])));
    // Player 1 owns the team and holds the upgrade; player 0 (the script player) does not.
    g.scripts_mut().set_player_name("player1", 1);
    g.apply_player_upgrade(1, "Upgrade_Big");
    let team = Dict(vec![
        ("teamName".to_owned(), DictValue::Ascii("Bravo".to_owned())),
        ("teamOwner".to_owned(), DictValue::Ascii("player1".to_owned())),
        ("teamUnitType1".to_owned(), DictValue::Ascii("Tank".to_owned())),
        ("teamUnitMinCount1".to_owned(), DictValue::Int(2)),
        ("teamUnitMaxCount1".to_owned(), DictValue::Int(2)),
        ("teamReinforcementOrigin".to_owned(), DictValue::Ascii("origin".to_owned())),
        ("teamIsSingleton".to_owned(), DictValue::Bool(false)),
    ]);
    g.scripts_mut().load_teams(&[team]);
    g.scripts_mut().set_waypoints([
        ("origin".to_owned(), Coord3D::new(50.0, 50.0, 0.0)),
        ("dest".to_owned(), Coord3D::new(250.0, 50.0, 0.0)),
    ]);
    g
}

fn run(g: &mut GameLogic, scripts: Vec<Script>) {
    g.scripts_mut().load(vec![(0, ScriptList { scripts, groups: Vec::new() })]);
    g.update();
}

fn tank_ids(g: &GameLogic) -> Vec<zr_sim::ObjectId> {
    g.objects().filter(|o| o.template.name == "Tank").map(|o| o.id).collect()
}

/// `ScriptActions::doCreateReinforcements` (`ScriptActions.cpp:499`) creates the team's units with
/// `newObject(thingTemplate, theTeam)` and then iterates `theTeam->iterate_TeamMemberList()` in the
/// same call. The port's `pending_team_joins` queue joined them only after `ScriptEngine::update`
/// returned, so `team_members` was empty for the dispatch loops: with no transport the
/// `MoveTo` got an empty id list and the reinforcements never walked, and with a transport nothing
/// was loaded or dispatched. The owner has to come from the team before the modules run, too
/// (`Object::getControllingPlayer` is the team's, Object.cpp:1620), or the new object misses its
/// owner's upgrade modules. Without the member-list insert the created team is also still empty at
/// the end of the pass and `TeamPrototype::updateState` removes it (`Team.cpp:1054-1088`), so the
/// whole reinforcement team disappears.
#[test]
fn reinforcements_belong_to_their_team_while_the_action_still_runs() {
    let mut g = world();
    run(&mut g, vec![one_shot("reinforce", vec![action("CREATE_REINFORCEMENT_TEAM", vec![sparam("Bravo"), sparam("dest")])])]);
    let ids = tank_ids(&g);
    assert_eq!(ids.len(), 2, "both units were created");
    let team = g.scripts().teams.values().find(|t| t.proto == "bravo").expect("the team still exists after its first pass");
    assert_eq!(team.members.len(), 2, "both units joined the team in the action's own pass");
    // The same action's dispatch (`ScriptActions.cpp:774-800`): no transport, origin != destination,
    // so the new members are ordered to walk to `dest`. With an empty member list no order is given.
    g.run_frames(30);
    for id in &ids {
        let o = g.find_object(*id).expect("the unit is alive");
        assert_eq!(o.player, 1, "the team's controlling player owns the new unit");
        assert!((o.body.as_ref().map_or(0.0, |b| b.max_health()) - 200.0).abs() < 0.01, "the owner's upgrade module ran on creation");
        assert!(o.position.x > 55.0, "the unit walks to the destination, at {:?}", o.position);
    }
}

/// `ScriptActions::createUnitOnTeamAt` (`ScriptActions.cpp:1164`) has the same owner problem, and its
/// unit is invisible to `TEAM_*` actions later in the same pass before the fix.
#[test]
fn a_named_script_unit_is_owned_and_in_the_team_at_creation() {    let mut g = world();
    run(
        &mut g,
        vec![one_shot(
            "named",
            vec![action(
                "CREATE_NAMED_ON_TEAM_AT_WAYPOINT",
                vec![sparam("Hero"), sparam("Tank"), sparam("Bravo"), sparam("origin")],
            )],
        )],
    );
    let id = g.scripts().named_object("Hero").expect("the named unit exists");
    let o = g.find_object(id).expect("alive");
    assert_eq!(o.player, 1, "the team's controlling player owns the new unit");
    assert!((o.body.as_ref().map_or(0.0, |b| b.max_health()) - 200.0).abs() < 0.01, "the owner's upgrade module ran on creation");
    let team = g.scripts().teams.values().find(|t| t.proto == "bravo").expect("the team still exists after its first pass");
    assert!(team.members.contains(&id), "the unit is in the member list at once");
}

/// `teamStartsFull` + `teamTransport`: `doCreateReinforcements` runs the `PartitionSolver` over
/// `theTeam->iterate_TeamMemberList()` (`ScriptActions.cpp:579-642`) and loads every solved unit with
/// `addToContain` in the same call. Before the fix the new units were not members yet, so the solver
/// had an empty entry list and the transport stayed empty (and then the whole team was removed as
/// empty at the end of the pass).
#[test]
fn a_starts_full_reinforcement_team_loads_its_transport_in_the_same_pass() {
    let mut transport = ObjectTemplate::new("Transport");
    transport.kind_of = kind_of::VEHICLE | kind_of::TRANSPORT | kind_of::SELECTABLE;
    transport.geometry.major_radius = 10.0;
    transport.max_health = 300.0;
    transport.initial_health = 300.0;
    transport.locomotor = Some(zr_sim::LocomotorParams::from_ini(zr_sim::LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    transport.contain = Some(Arc::new(zr_sim::ContainTemplate {
        kind: zr_sim::ContainKind::Transport,
        capacity: 5,
        allow_inside_kind_of: None,
        forbid_inside_kind_of: 0,
        allow_allies_inside: true,
        allow_enemies_inside: false,
        allow_neutral_inside: false,
        passengers_allowed_to_fire: false,
        damage_percentage_to_units: 0.0,
        number_of_exit_paths: 1,
        exit_delay: 0,
        door_open_time: 1,
        scatter_nearby_on_exit: false,
        orient_like_container_on_exit: false,
        reset_mood_check_on_exit: false,
        health_regen: 0.0,
        do_i_heal_objects: false,
        heal_frames: 0.0,
        initial_payload: Vec::new(),
        tunnel_frames_for_full_heal: 1.0,
        is_enclosing_container: false,
        fire_points: zr_sim::GarrisonFirePoints::default(),
        passengers_in_turret: false,
        weapon_bonus_passed_to_passengers: false,
        immune_to_clear_building_attacks: false,
    }));
    let mut g = world();
    // Add the transport to the template store the world installed; the tank takes one slot
    // (`TransportSlotCount`, otherwise the engine's "lots of room" `0x7fffff`).
    let mut tank = (*g.find_template("Tank").unwrap()).clone();
    tank.transport_slot_count = 1;
    let store = Store(vec![Arc::new(tank), Arc::new(transport)]);
    g.set_template_source(Arc::new(store));
    let d = Dict(vec![
        ("teamName".to_owned(), DictValue::Ascii("Bravo".to_owned())),
        ("teamOwner".to_owned(), DictValue::Ascii("player1".to_owned())),
        ("teamUnitType1".to_owned(), DictValue::Ascii("Tank".to_owned())),
        ("teamUnitMinCount1".to_owned(), DictValue::Int(2)),
        ("teamUnitMaxCount1".to_owned(), DictValue::Int(2)),
        ("teamReinforcementOrigin".to_owned(), DictValue::Ascii("origin".to_owned())),
        ("teamTransport".to_owned(), DictValue::Ascii("Transport".to_owned())),
        ("teamStartsFull".to_owned(), DictValue::Bool(true)),
        ("teamIsSingleton".to_owned(), DictValue::Bool(false)),
    ]);
    g.scripts_mut().load_teams(&[d]);
    run(&mut g, vec![one_shot("reinforce", vec![action("CREATE_REINFORCEMENT_TEAM", vec![sparam("Bravo"), sparam("dest")])])]);
    let transport = g.objects().find(|o| o.template.name == "Transport").map(|o| o.id).expect("the team's transport was created");
    let ids = tank_ids(&g);
    assert_eq!(ids.len(), 2);
    for id in ids {
        assert_eq!(g.find_object(id).unwrap().contained_by, Some(transport), "the unit was loaded in the action's own pass");
    }
}
