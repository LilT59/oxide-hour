//! Lane 123: the attack order sets the AI's last command source (`privateAttackObject` /
//! `privateAttackPosition`, AIUpdate.cpp:3442/:3577), so a player's attack drops a temporary stealth
//! grant (`StealthUpdate.cpp:720-729`). Synthetic data only.

use std::sync::Arc;

use zr_sim::ai_states::CommandSource;
use zr_sim::behaviors::StealthTemplate;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(400, 400, 0)
}

fn gun() -> WeaponTemplate {
    let mut w = WeaponTemplate::new("Rifle");
    w.primary_damage = 1.0;
    w.attack_range = 100.0;
    w.min_delay_between_shots = 30;
    w.max_delay_between_shots = 30;
    w
}

/// A wheeled unit with a weapon, so the attack-order command accepts it
/// (`AIGroup::groupAttackObjectPrivate` orders only members that can attack).
fn unit(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 200.0;
    t.initial_health = 200.0;
    t.vision_range = 200.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(gun()));
    t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    t
}

/// A `StealthUpdate` that starts off (`InnateStealth = No`) and is woken by a grant
/// (`GrantedBySpecialPower`, StealthUpdate.cpp:132, :154-157).
fn granted_stealth() -> StealthTemplate {
    StealthTemplate {
        stealth_delay: 0,
        move_threshold_speed: 0.0,
        forbidden_conditions: 0,
        required_status: 0,
        forbidden_status: 0,
        innate_stealth: false,
        granted_by_special_power: true,
        disguises_as_team: false,
        reveal_distance_from_target: 0.0,
        order_idle_enemies_to_attack_on_reveal: false,
        use_rider_stealth: false,
    }
}

fn source(g: &GameLogic, id: ObjectId) -> CommandSource {
    g.find_object(id).expect("object").ai.machine.last_command_source
}

/// `privateAttackObject` (`AIUpdate.cpp:3442`): a command writes `m_lastCommandSource` before
/// `setState(AI_ATTACK_OBJECT)`. The port's `ai_start_attack` did not, so a player's attack left the
/// AI's last *move* source in place (the assertion fails with the source write removed).
#[test]
fn a_player_attack_order_sets_the_last_command_source() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let attacker = g.create_object(&Arc::new(unit("Attacker")), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let victim = g.create_object(&Arc::new(unit("Victim")), Coord3D::new(250.0, 200.0, 0.0), 0.0, 1);
    // The object's source is the AI's until a command writes it (`m_lastCommandSource` starts at
    // `CMD_FROM_AI`, AIUpdate.cpp:254).
    assert_eq!(source(&g, attacker), CommandSource::Ai);

    g.submit(0, Command::Attack { ids: vec![attacker], target: victim });
    g.run_frames(1);
    assert_eq!(g.find_object(attacker).unwrap().ai.machine.current, zr_sim::ai_states::AiStateType::AttackObject);
    assert_eq!(source(&g, attacker), CommandSource::Player, "a player attack records CMD_FROM_PLAYER");
}

/// "If the last AI command given was by the player... then LOSE the stealth now!"
/// (`StealthUpdate.cpp:720-729`): an attack order is a player command, so the temporary grant is
/// dropped at the next stealth update. Revert check: without the source write the grant survives.
#[test]
fn a_player_attack_order_drops_a_temporary_stealth_grant() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let mut t = unit("Saboteur");
    t.stealth = Some(granted_stealth());
    let saboteur = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let victim = g.create_object(&Arc::new(unit("Victim")), Coord3D::new(260.0, 200.0, 0.0), 0.0, 1);
    g.run_frames(1);
    g.stealth_receive_grant(saboteur, true, 20);
    assert!(g.is_stealthed(saboteur), "the grant turns the stealth on");

    g.submit(0, Command::Attack { ids: vec![saboteur], target: victim });
    g.run_frames(2);
    let o = g.find_object(saboteur).unwrap();
    assert_eq!(source(&g, saboteur), CommandSource::Player);
    assert_eq!(o.behavior.stealth_frames_granted, 0, "the player command drops the grant");
    assert!(!g.is_stealthed(saboteur), "the unit is no longer stealthed");
}
