//! Lane 123: the attack sub-machine's incidental turret scan during the initial approach
//! (`AIAttackApproachTargetState::update` / `AIAttackPursueTargetState::update`, AIStates.cpp:2783-2794,
//! :3108-3123). Synthetic data only.

use std::sync::Arc;

use zr_sim::ai_states::AiStateType;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::turret_ai::TurretTarget;
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(400, 400, 0)
}

fn gun(range: f32) -> WeaponTemplate {
    let mut w = WeaponTemplate::new("Gun");
    w.primary_damage = 10.0;
    w.attack_range = range;
    w.min_delay_between_shots = 30;
    w.max_delay_between_shots = 30;
    w
}

/// A turreted vehicle: the current weapon sits on the turret (`turret_controlled_slots` 0 = every slot).
fn turreted(name: &str, range: f32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 300.0;
    t.initial_health = 300.0;
    t.vision_range = 300.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(gun(range)));
    t.combat = CombatTemplate {
        weapon_set: Some(set),
        turret_turn_rate: Some(0.1),
        turret_pitch_rate: Some(0.05),
        ..CombatTemplate::default()
    };
    t
}

fn plain(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 300.0;
    t.initial_health = 300.0;
    t
}

/// `AIAttackApproachTargetState::update` (`AIStates.cpp:2783-2794`): while the attack is in its initial
/// approach and the current weapon is on a turret, `getNextMoodTarget(true, false)` looks for an
/// incidental target and makes it the turret's. The ordered victim is far out of range, so only that
/// scan can put the close enemy on the turret.
/// Revert check: remove the scan and the turret keeps the ordered victim (the attack pass sets it).
#[test]
fn a_turret_aims_at_an_incidental_target_during_the_initial_approach() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    g.reveal_map_permanently_for_player(0);
    let a = g.create_object(&Arc::new(turreted("Tank", 100.0)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    // Out of weapon range (100) at 400 units.
    let far = g.create_object(&Arc::new(plain("Far")), Coord3D::new(600.0, 200.0, 0.0), 0.0, 1);
    // Within the gun's range and the tank's vision, so the turret can shoot it.
    let near = g.create_object(&Arc::new(plain("Near")), Coord3D::new(260.0, 200.0, 0.0), 0.0, 1);

    g.submit(0, Command::Attack { ids: vec![a], target: far });
    let mut incidental = None;
    for _ in 0..6 {
        g.run_frames(1);
        let o = g.find_object(a).unwrap();
        assert_eq!(o.ai.machine.current, AiStateType::AttackObject, "the ordered attack runs");
        assert_eq!(o.ai.machine.goal_object, far, "the attack's goal stays the ordered victim");
        if let Some(m) = o.combat.turret.as_ref() {
            if m.target == TurretTarget::Object(near) {
                incidental = Some(g.frame());
                break;
            }
        }
    }
    assert!(incidental.is_some(), "the turret picked the incidental target it could shoot");
}
