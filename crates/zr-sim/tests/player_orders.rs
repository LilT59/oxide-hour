//! The player-order commands ported in `zr_sim::player_orders`: force move, waypoints, scatter,
//! overcharge, weapon switch, upgrade cancel and the Chinook combat drop. Synthetic data only.
//!
//! Ports under test: `GameLogicDispatch.cpp` (`onDoForcemoveto`, `onAddWaypoint`, `onDoScatter`,
//! `onToggleOvercharge`, `onDoSwitchWeapons`, `onCancelUpgrade`, `onCombatdrop*`),
//! `AIGroup.cpp` (`groupMoveToPosition`, `groupScatter`, `groupToggleOvercharge`,
//! `setWeaponLockForGroup`, `groupCombatDrop`) and `ProductionUpdate.cpp` (`cancelUpgrade`).

use std::collections::BTreeMap;
use std::sync::Arc;

use zr_sim::chinook::ChinookTemplate;
use zr_sim::contain::{ContainKind, ContainTemplate, GarrisonFirePoints};
use zr_sim::modules2::OverchargeTemplate;
use zr_sim::production::ProductionTemplate;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::upgrade::UpgradeInfo;
use zr_sim::weapon::{WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TemplateSource, TerrainLogic};

struct FixedStore(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for FixedStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

fn mobile(name: &str, radius: f32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = radius;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 300.0, 300.0, 360.0));
    t
}

fn structure(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    t.max_health = 500.0;
    t.initial_health = 500.0;
    t.geometry.major_radius = 15.0;
    t
}

fn contain() -> ContainTemplate {
    ContainTemplate {
        kind: ContainKind::Open,
        capacity: -1,
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
        scatter_nearby_on_exit: true,
        orient_like_container_on_exit: false,
        reset_mood_check_on_exit: false,
        health_regen: 0.0,
        do_i_heal_objects: false,
        heal_frames: 0.0,
        initial_payload: Vec::new(),
        tunnel_frames_for_full_heal: 1.0,
        is_enclosing_container: false,
        fire_points: GarrisonFirePoints::default(),
        passengers_in_turret: false,
        weapon_bonus_passed_to_passengers: false,
        immune_to_clear_building_attacks: false,
    }
}

fn gun(name: &str) -> WeaponTemplate {
    let mut w = WeaponTemplate::new(name);
    w.primary_damage = 10.0;
    w.attack_range = 100.0;
    w.min_delay_between_shots = 5;
    w.max_delay_between_shots = 5;
    w
}

fn world(templates: Vec<ObjectTemplate>) -> GameLogic {
    let store = FixedStore(templates.into_iter().map(Arc::new).collect());
    let mut g = GameLogic::new(TerrainLogic::flat(500, 500, 0), 1);
    g.add_player(0, 0);
    g.set_template_source(Arc::new(store));
    g
}

/// `MSG_DO_FORCEMOVETO`: the same group move as `MSG_DO_MOVETO`; units keep their offset from the
/// group centre. Both units are 56.6 units from the click, so the near-to-far order is the reverse
/// member order (`SimpleObjectIterator::insert` prepends and the sort is stable), and the centre is the
/// *first* unit's position (`AIGroup.cpp:1734-1743`), here the right-hand one.
#[test]
fn force_move_is_the_group_move() {
    let mut g = world(vec![mobile("Tank", 5.0)]);
    let a = g.create_object(&g.find_template("Tank").unwrap(), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    let b = g.create_object(&g.find_template("Tank").unwrap(), Coord3D::new(90.0, 10.0, 0.0), 0.0, 0);
    g.submit(0, Command::ForceMove { ids: vec![a, b], pos: Coord3D::new(50.0, 50.0, 0.0) });
    g.run_frames(1);
    // centre (90,10) = the first unit in near-to-far order; the left unit is offset 80, clamped to 6*r = 30.
    let ga = g.find_object(a).unwrap().ai.machine.goal_position;
    let gb = g.find_object(b).unwrap().ai.machine.goal_position;
    // Traced by hand against the C++ (AIGroup.cpp:546-548 -> adjustDestination, AIPathfind.cpp:5462-5540 -> checkForAdjust
    // :5314 -> tightenPath :8387 with iterateCellsAlongLine :9196). The Tank is one centred cell (radius 0), so the offsets
    // are cell centres. The centre (b, dest (50,50)): cell (5,5); its tightening walks the degenerate line (5,5)->(5,5)
    // and emits (5,5) and (5,6) (the C++ Bresenham steps y when the line has no length), and the last accepted cell is
    // (5,6), centre (55,65); checkPathCost is 10 against 1.4*(5+15)=28, so it stands. The left unit (a, dest (20,50)):
    // cell (2,5), tightened along (2,5)..(5,5) to (5,5) (b's cell is (5,6), so (5,5) is free), checkPathCost 0, so (55,55).
    assert_eq!((ga.x, ga.y), (55.0, 55.0), "left unit goal {ga:?}");
    assert_eq!((gb.x, gb.y), (55.0, 65.0), "right unit goal {gb:?}");
    assert_eq!(g.find_object(a).unwrap().ai.machine.current, zr_sim::ai_states::AiStateType::MoveTo);
}

/// `MSG_ADD_WAYPOINT`: a second and third waypoint append to the running path
/// (`AIGroup::groupMoveToPosition(addWaypoint = true)` -> `aiFollowPathAppend`).
#[test]
fn add_waypoint_appends_to_the_goal_path() {
    let mut g = world(vec![mobile("Tank", 5.0)]);
    let u = g.create_object(&g.find_template("Tank").unwrap(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.submit(0, Command::MoveTo { ids: vec![u], pos: Coord3D::new(400.0, 50.0, 0.0) });
    g.submit(0, Command::AddWaypoint { ids: vec![u], pos: Coord3D::new(400.0, 200.0, 0.0) });
    g.submit(0, Command::AddWaypoint { ids: vec![u], pos: Coord3D::new(400.0, 350.0, 0.0) });
    g.run_frames(1);
    let o = g.find_object(u).unwrap();
    assert_eq!(o.ai.machine.current, zr_sim::ai_states::AiStateType::FollowPath, "converted to a waypoint path");
    assert_eq!(o.ai.machine.goal_path.len(), 3, "path {:?}", o.ai.machine.goal_path);
    // Traced by hand against the C++ (as in force_move_is_the_group_move): (400,200) -> cell (40,20), centre (405,205);
    // tightening walks the degenerate line (40,20)->(40,20), emitting (40,20) and (40,21), last accepted (40,21) = (405,215);
    // checkPathCost 10 < 1.4*20 = 28, so it stands. (400,350) -> (405,365) by the same steps.
    assert_eq!((o.ai.machine.goal_path[1].x, o.ai.machine.goal_path[1].y), (405.0, 215.0), "second waypoint {:?}", o.ai.machine.goal_path[1]);
    assert_eq!((o.ai.machine.goal_path[2].x, o.ai.machine.goal_path[2].y), (405.0, 365.0), "third waypoint {:?}", o.ai.machine.goal_path[2]);
}

/// `MSG_ADD_WAYPOINT` when idle: a single-point path (`privateFollowPathAppend`'s else branch).
#[test]
fn add_waypoint_from_idle_starts_a_path() {
    let mut g = world(vec![mobile("Tank", 5.0)]);
    let u = g.create_object(&g.find_template("Tank").unwrap(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.submit(0, Command::AddWaypoint { ids: vec![u], pos: Coord3D::new(250.0, 50.0, 0.0) });
    g.run_frames(1);
    let o = g.find_object(u).unwrap();
    assert_eq!(o.ai.machine.current, zr_sim::ai_states::AiStateType::FollowPath);
    assert_eq!(o.ai.machine.goal_path.len(), 1);
    // Traced by hand as above: (250,50) -> cell (25,5), centre (255,55); the degenerate tightening emits (25,5) and (25,6),
    // last accepted (25,6) = (255,65); checkPathCost 10 < 1.4*20 = 28, so it stands.
    assert_eq!((o.ai.machine.goal_path[0].x, o.ai.machine.goal_path[0].y), (255.0, 65.0), "adjusted waypoint {:?}", o.ai.machine.goal_path[0]);
}

/// `MSG_DO_SCATTER`: each mobile unit moves four bounding radii outward from the group centre.
#[test]
fn scatter_moves_units_outward() {
    let mut g = world(vec![mobile("Tank", 5.0)]);
    let a = g.create_object(&g.find_template("Tank").unwrap(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&g.find_template("Tank").unwrap(), Coord3D::new(200.0, 100.0, 0.0), 0.0, 0);
    g.submit(0, Command::Scatter { ids: vec![a, b] });
    g.run_frames(1);
    // centre (150,100); 4*r = 20 outward.
    let ga = g.find_object(a).unwrap().ai.machine.goal_position;
    let gb = g.find_object(b).unwrap().ai.machine.goal_position;
    assert!((ga.x - 80.0).abs() < 1.0 && (ga.y - 100.0).abs() < 1.0, "left goal {ga:?}");
    assert!((gb.x - 220.0).abs() < 1.0 && (gb.y - 100.0).abs() < 1.0, "right goal {gb:?}");
}

/// `MSG_TOGGLE_OVERCHARGE`: toggles a power plant's `OverchargeBehavior`; a unit without one is
/// untouched (`AIGroup::groupToggleOvercharge`).
#[test]
fn toggle_overcharge_toggles_the_behavior() {
    let mut plant = structure("PowerPlant");
    plant.overcharge = Some(OverchargeTemplate {
        health_percent_to_drain_per_second: 0.0,
        not_allowed_when_health_below_percent: 0.0,
        rods_extend_time: 30,
    });
    let mut g = world(vec![plant, mobile("Tank", 5.0)]);
    let p = g.create_object(&g.find_template("PowerPlant").unwrap(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let t = g.create_object(&g.find_template("Tank").unwrap(), Coord3D::new(100.0, 50.0, 0.0), 0.0, 0);
    assert!(!g.find_object(p).unwrap().behavior.overcharge);
    g.submit(0, Command::ToggleOvercharge { ids: vec![p, t] });
    g.run_frames(1);
    assert!(g.find_object(p).unwrap().behavior.overcharge, "plant toggled on");
    assert!(!g.find_object(t).unwrap().behavior.overcharge, "tank has no overcharge");
    g.submit(0, Command::ToggleOvercharge { ids: vec![p] });
    g.run_frames(1);
    assert!(!g.find_object(p).unwrap().behavior.overcharge, "plant toggled off");
}

/// `MSG_SWITCH_WEAPONS`: permanently locks the chosen slot (`WeaponSet::setWeaponLock`,
/// `LOCKED_PERMANENTLY`); a switch to a missing slot is ignored.
#[test]
fn switch_weapons_locks_permanently() {
    let mut t = mobile("Tank", 5.0);
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(gun("Cannon")));
    set.weapons[1] = Some(Arc::new(gun("Missile")));
    t.combat.weapon_set = Some(set);
    let mut g = world(vec![t]);
    let u = g.create_object(&g.find_template("Tank").unwrap(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    assert_eq!(g.find_object(u).unwrap().combat.weapons.permanent_lock, None);
    g.submit(0, Command::SwitchWeapons { ids: vec![u], slot: 1 });
    g.run_frames(1);
    assert_eq!(g.find_object(u).unwrap().combat.weapons.permanent_lock, Some(1));
    g.submit(0, Command::SwitchWeapons { ids: vec![u], slot: 3 });
    g.run_frames(1);
    assert_eq!(g.find_object(u).unwrap().combat.weapons.permanent_lock, Some(1), "missing slot ignored");
}

/// `MSG_CANCEL_UPGRADE`: refunds the queued upgrade and drops its entry
/// (`ProductionUpdate::cancelUpgrade`).
#[test]
fn cancel_upgrade_refunds_and_drops_the_entry() {
    let mut factory = structure("Factory");
    factory.production = Some(ProductionTemplate::default());
    let mut g = world(vec![factory]);
    g.add_player(0, 1000);
    let mut info = BTreeMap::new();
    info.insert("UpgradeA".to_string(), UpgradeInfo { cost: 300, build_time_frames: 30 });
    info.insert("UpgradeB".to_string(), UpgradeInfo { cost: 100, build_time_frames: 30 });
    g.set_upgrade_info(info);
    let f = g.create_object(&g.find_template("Factory").unwrap(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);

    assert!(g.queue_upgrade(0, f, "UpgradeA"));
    assert_eq!(g.player(0).unwrap().money, 700);

    // A different upgrade is not cancelled.
    g.submit(0, Command::CancelUpgrade { building: f, upgrade: "UpgradeB".into() });
    g.run_frames(1);
    assert_eq!(g.player(0).unwrap().money, 700, "nothing matched");

    g.submit(0, Command::CancelUpgrade { building: f, upgrade: "UpgradeA".into() });
    g.run_frames(1);
    assert_eq!(g.player(0).unwrap().money, 1000, "refunded the cost");
    assert!(g.find_object(f).unwrap().production.as_ref().unwrap().queue.is_empty());

    // A second cancel finds nothing and is a no-op.
    g.submit(0, Command::CancelUpgrade { building: f, upgrade: "UpgradeA".into() });
    g.run_frames(1);
    assert_eq!(g.player(0).unwrap().money, 1000);

    // A non-owner cannot cancel another player's upgrade.
    g.add_player(1, 1000);
    assert!(g.queue_upgrade(0, f, "UpgradeA"));
    g.submit(1, Command::CancelUpgrade { building: f, upgrade: "UpgradeA".into() });
    g.run_frames(1);
    assert_eq!(g.player(0).unwrap().money, 700, "still queued after the enemy's cancel");
}

/// `MSG_COMBATDROP_AT_LOCATION` / `_AT_OBJECT`: a Chinook starts its drop at the target.
#[test]
fn combat_drop_starts_the_chinook_drop() {
    let mut chinook = mobile("Chinook", 12.0);
    chinook.chinook = Some(ChinookTemplate::default());
    chinook.contain = Some(Arc::new(contain()));
    let mut g = world(vec![chinook, structure("Bunker")]);
    let c = g.create_object(&g.find_template("Chinook").unwrap(), Coord3D::new(50.0, 50.0, 200.0), 0.0, 0);
    let b = g.create_object(&g.find_template("Bunker").unwrap(), Coord3D::new(300.0, 300.0, 0.0), 0.0, 0);

    assert!(g.chinook_state(c).is_none());
    g.submit(0, Command::CombatDrop { ids: vec![c], target: zr_sim::CombatDropTarget::Object(b) });
    g.run_frames(1);
    let s = g.chinook_state(c).expect("combat drop started");
    assert_eq!(s.target_obj, Some(b));
    assert!(s.target_pos.z > 0.0, "hover height above ground");

    // A second Chinook ordered to a bare location.
    let d = g.create_object(&g.find_template("Chinook").unwrap(), Coord3D::new(50.0, 50.0, 200.0), 0.0, 0);
    g.submit(0, Command::CombatDrop { ids: vec![d], target: zr_sim::CombatDropTarget::Position(Coord3D::new(100.0, 100.0, 0.0)) });
    g.run_frames(1);
    let s2 = g.chinook_state(d).expect("ground combat drop started");
    assert_eq!(s2.target_obj, None);
    assert_eq!(s2.target_pos.x, 100.0);
}

/// `AIGroup::groupAttackMoveToPosition` -> `privateAttackMoveToPosition` (`AIUpdate.cpp:3604`) clips a player's goal
/// to the map extent trimmed by half a partition cell (`setGoalPositionClipped`, `:351-380`).
#[test]
fn attack_move_goal_is_clipped_to_the_map() {
    let mut t = mobile("Tank", 5.0);
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(gun("Cannon")));
    t.combat.weapon_set = Some(set);
    let mut g = world(vec![t]);
    let u = g.create_object(&g.find_template("Tank").unwrap(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.submit(0, Command::AttackMove { ids: vec![u], pos: Coord3D::new(-200.0, 50.0, 0.0) });
    g.run_frames(1);
    let o = g.find_object(u).unwrap();
    assert_eq!(o.ai.machine.current, zr_sim::ai_states::AiStateType::AttackMoveTo);
    // fudge = m_partitionCellSize * 0.5 with the 40-unit default cell: the low edge is x = 20.
    assert!((o.ai.machine.goal_position.x - 20.0).abs() < 1e-3, "clipped goal {:?}", o.ai.machine.goal_position);
    assert_eq!(o.ai.machine.goal_position.y, 50.0);
}

/// `privateMoveToPosition` and `privateAttackMoveToPosition` return when `!isMobile()` (`AIUpdate.cpp:2889-2893`,
/// `:3595-3598`): a disabled unit drops the new order and keeps its old state and goal.
#[test]
fn disabled_unit_drops_move_and_attack_move_orders() {
    let mut g = world(vec![mobile("Tank", 5.0)]);
    let u = g.create_object(&g.find_template("Tank").unwrap(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.submit(0, Command::MoveTo { ids: vec![u], pos: Coord3D::new(100.0, 50.0, 0.0) });
    g.run_frames(1);
    assert_eq!(g.find_object(u).unwrap().ai.machine.current, zr_sim::ai_states::AiStateType::MoveTo);
    let kept = g.find_object(u).unwrap().ai.machine.goal_position;

    g.set_disabled(u, zr_sim::disabled::DisabledType::Emp);
    g.submit(0, Command::MoveTo { ids: vec![u], pos: Coord3D::new(300.0, 300.0, 0.0) });
    g.run_frames(1);
    let o = g.find_object(u).unwrap();
    assert_eq!(o.ai.machine.goal_position, kept, "move dropped while disabled");
    assert_eq!(o.ai.machine.current, zr_sim::ai_states::AiStateType::MoveTo);

    g.submit(0, Command::AttackMove { ids: vec![u], pos: Coord3D::new(300.0, 300.0, 0.0) });
    g.run_frames(1);
    let o = g.find_object(u).unwrap();
    assert_eq!(o.ai.machine.goal_position, kept, "attack-move dropped while disabled");
    assert_ne!(o.ai.machine.current, zr_sim::ai_states::AiStateType::AttackMoveTo);
}

/// `privateAttackArea` (`AIUpdate.cpp:3671-3680`) sets the area the state hunts in and no goal position: the
/// unit's goal is left where it was (`AIAttackAreaState` reads `getAreaToGuard`, `AIStates.cpp:7392`).
#[test]
fn attack_area_sets_no_goal_position() {
    let mut g = world(vec![mobile("Tank", 5.0)]);
    let u = g.create_object(&g.find_template("Tank").unwrap(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let before = g.find_object(u).unwrap().ai.machine.goal_position;
    g.command_attack_area(0, &[u], Coord3D::new(300.0, 300.0, 0.0), 100.0);
    g.run_frames(1);
    assert_eq!(g.find_object(u).unwrap().ai.machine.goal_position, before, "no goal for the attack area");
}
