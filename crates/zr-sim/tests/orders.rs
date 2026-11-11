//! `ForceAttack` on synthetic data (`MSG_DO_FORCE_ATTACK_OBJECT`).

use std::sync::Arc;
use zr_sim::armor::ArmorTemplate;
use zr_sim::damage::DamageType;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(100, 100, 0)
}

fn cannon() -> WeaponTemplate {
    let mut w = WeaponTemplate::new("Cannon");
    w.primary_damage = 40.0;
    w.damage_type = DamageType::ArmorPiercing;
    w.attack_range = 150.0;
    w.min_delay_between_shots = 30;
    w.max_delay_between_shots = 30;
    w
}

fn tank(name: &str) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.geometry.height = 10.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.vision_range = 200.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    let mut armor = ArmorTemplate::new("TankArmor");
    armor.coefficients[DamageType::ArmorPiercing.index()] = 0.5;
    t.combat = CombatTemplate {
        armor: Some(Arc::new(armor)),
        weapon_set: {
            let mut s = WeaponSetTemplate::default();
            s.weapons[0] = Some(Arc::new(cannon()));
            Some(s)
        },
        ..CombatTemplate::default()
    };
    Arc::new(t)
}

#[test]
fn force_attack_targets_an_ally_but_plain_attack_does_not() {
    let mut g = GameLogic::new(flat(), 1);
    let attacker = g.create_object(&tank("A"), Coord3D::new(20.0, 20.0, 0.0), 0.0, 0);
    let ally = g.create_object(&tank("B"), Coord3D::new(40.0, 20.0, 0.0), 0.0, 0);

    // A normal attack order on an ally is rejected.
    g.submit(0, Command::Attack { ids: vec![attacker], target: ally });
    g.run_frames(1);
    assert!(g.find_object(attacker).unwrap().combat.attack.is_none(), "plain attack ignores an ally");

    // Force-attack accepts it.
    g.submit(0, Command::ForceAttack { ids: vec![attacker], target: ally });
    g.run_frames(1);
    assert!(g.find_object(attacker).unwrap().combat.attack.is_some(), "force attack takes an ally target");
    assert!(g.find_object(attacker).unwrap().combat.force_attack);
}

#[test]
fn force_attack_still_rejects_self() {
    let mut g = GameLogic::new(flat(), 1);
    let a = g.create_object(&tank("A"), Coord3D::new(20.0, 20.0, 0.0), 0.0, 0);
    g.submit(0, Command::ForceAttack { ids: vec![a], target: a });
    g.run_frames(1);
    assert!(g.find_object(a).unwrap().combat.attack.is_none());
}
