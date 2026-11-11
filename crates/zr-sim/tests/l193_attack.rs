//! Lane 193 (round 3): the attack leaves in the update of the shot that kills its victim. The fire state's
//! transition into AIM runs AIM's `onEnter` (CONTINUE), and then AIM's conditions, in the same
//! `StateMachine::internalSetState` call (StateMachine.cpp:626-660, :124-194). `cannotPossiblyAttackObject`
//! (AIStates.cpp:1242) fails the attack for a dead victim, so the unit is `AI_IDLE` on the frame the victim dies.
//! Synthetic data only.

use std::sync::Arc;
use zr_sim::ai_states::AiStateType;
use zr_sim::math::Coord3D;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, DamageType, GameLogic, ObjectTemplate, TerrainLogic};

fn unit(name: &str, damage: f32) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.geometry.height = 10.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.vision_range = 300.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    let mut w = WeaponTemplate::new("Gun");
    w.primary_damage = damage;
    w.damage_type = DamageType::Explosion;
    w.primary_damage_radius = 1.0;
    w.secondary_damage_radius = 1.0;
    w.attack_range = 100.0;
    w.min_delay_between_shots = 10;
    w.max_delay_between_shots = 10;
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(w));
    t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    Arc::new(t)
}

/// Each listed player's default team (`PlayerList::newGame` -> `TeamFactory::initFromSides`, Player.cpp:760-770). A
/// synthetic world has no sides, so its objects are on no team unless this makes them (as skirmish_ai's `with_default_team`).
fn give_default_teams(g: &mut GameLogic, players: &[u8]) {
    let names: Vec<(String, String)> = players.iter().map(|p| (format!("teamP{p}"), format!("p{p}"))).collect();
    for p in players {
        g.scripts_mut().set_player_name(&format!("p{p}"), *p);
    }
    let protos: Vec<(&str, &str)> = names.iter().map(|(t, p)| (t.as_str(), p.as_str())).collect();
    g.scripts_mut().load_default_team_protos(&protos);
    let sides: Vec<(&str, u8)> = names.iter().zip(players).map(|((t, _), p)| (t.as_str(), *p)).collect();
    g.init_from_sides(&sides);
}

/// `AIAttackState::update` (AIStates.cpp:5633-5669): the victim's team changed since the attack began, and the attacker
/// no longer considers it an enemy (neutral here), so the attack fails. A victim that stays an enemy keeps the attack.
#[test]
fn an_attack_ends_when_the_victims_team_turns_neutral() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.add_player(4, 0);
    g.set_enemies(0, 1);
    give_default_teams(&mut g, &[0, 1, 4]);
    // Non-zero damage: a zero-damage weapon is not a choosable weapon, so the attack would not start.
    let a = g.create_object(&unit("A", 1.0), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    let b = g.create_object(&unit("B", 0.0), Coord3D::new(30.0, 10.0, 0.0), 0.0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    g.run_frames(3);
    assert_eq!(g.find_object(a).unwrap().ai.machine.current, AiStateType::AttackObject, "the attack is under way");
    // Player 4 is neutral to player 0: the victim's team changes and it is no longer an enemy.
    g.set_object_team(b, 4);
    g.run_frames(2);
    assert_eq!(g.find_object(a).unwrap().ai.machine.current, AiStateType::Idle, "the attack ended on the team change");
}

/// `WeaponSet::getAttackResult` (WeaponSet.cpp:572) reads `getRelationship(victim)`, and an undetected defector reads as an
/// ally to every other object (Object.cpp:1603-1604). So a victim in its defection window is not a legal target: the
/// attack ends (the port's `target_info` reads the victim's flag). Victim owner unchanged, so only the flag can end it.
#[test]
fn an_attack_drops_a_victim_in_its_defection_window() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    give_default_teams(&mut g, &[0, 1]);
    let a = g.create_object(&unit("A", 1.0), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    let b = g.create_object(&unit("B", 0.0), Coord3D::new(30.0, 10.0, 0.0), 0.0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    g.run_frames(3);
    assert_eq!(g.find_object(a).unwrap().ai.machine.current, AiStateType::AttackObject, "the attack is under way");
    g.set_undetected_defector(b, true);
    g.run_frames(2);
    assert_eq!(g.find_object(a).unwrap().ai.machine.current, AiStateType::Idle, "the defector is not a legal target");
}

/// A one-clip attacker: its only weapon spends its single round and does not reload by itself.
fn one_shot_unit(name: &str, damage: f32) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.geometry.height = 10.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.vision_range = 300.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    let mut w = WeaponTemplate::new("Gun");
    w.primary_damage = damage;
    w.damage_type = DamageType::Explosion;
    w.primary_damage_radius = 1.0;
    w.secondary_damage_radius = 1.0;
    w.attack_range = 100.0;
    w.min_delay_between_shots = 10;
    w.max_delay_between_shots = 10;
    w.clip_size = 1;
    w.auto_reloads_clip = false;
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(w));
    t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    Arc::new(t)
}

/// `AIAttackState::update` chooses the weapon each update (AIStates.cpp:5686-5688); `chooseWeapon` skips a spent
/// weapon that does not reload (`WeaponSet.cpp`). The shot's transition check tests only the victim (F9), so the
/// attack is still under way in the shot's frame and ends on the next update.
#[test]
fn a_spent_weapon_ends_the_attack_one_update_after_the_shot() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    give_default_teams(&mut g, &[0, 1]);
    let a = g.create_object(&one_shot_unit("A", 1.0), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    let b = g.create_object(&unit("B", 0.0), Coord3D::new(30.0, 10.0, 0.0), 0.0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    let mut shot = None;
    for _ in 0..200 {
        g.run_frames(1);
        if g.find_object(b).unwrap().health() < 100.0 {
            shot = Some(g.frame());
            break;
        }
    }
    assert!(shot.is_some(), "the one round was fired");
    assert_eq!(g.find_object(a).unwrap().ai.machine.current, AiStateType::AttackObject, "the shot's frame keeps the attack");
    g.run_frames(1);
    assert_eq!(g.find_object(a).unwrap().ai.machine.current, AiStateType::Idle, "the next update finds no choosable weapon");
}

#[test]
fn the_attack_ends_in_the_frame_of_the_shot_that_kills_its_victim() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    // The attacker's shot is lethal (the victim has 100 hit points).
    let a = g.create_object(&unit("A", 500.0), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    let b = g.create_object(&unit("B", 0.0), Coord3D::new(30.0, 10.0, 0.0), 0.0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    let mut died_at = None;
    for _ in 0..200 {
        g.run_frames(1);
        let dead = g.find_object(b).is_none_or(|o| o.is_dying());
        if dead {
            died_at = Some(g.frame());
            break;
        }
    }
    let frame = died_at.expect("the victim died");
    let ai = g.find_object(a).unwrap().ai.machine.current;
    assert_eq!(ai, AiStateType::Idle, "the attack ended in the frame {frame} of the lethal shot, not one frame later");
}
