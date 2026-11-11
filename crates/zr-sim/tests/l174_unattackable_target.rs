//! Lane 174: `WeaponSet::getAttackResult`'s victim checks for `KINDOF_UNATTACKABLE` and
//! `OBJECT_STATUS_MASKED` (`WeaponSet.cpp:490-496`). AN1 seed 1's first meaningful divergence was a
//! China BattleMaster auto-acquiring an America `SpySatellitePing` (`KindOf = NO_COLLIDE IMMOBILE
//! UNATTACKABLE INERT`) at f3613, which the C++ refuses as a target and therefore stays Idle.

use std::sync::Arc;

use zr_sim::ai_states::AiStateType;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn gun() -> WeaponTemplate {
    let mut w = WeaponTemplate::new("Gun");
    w.primary_damage = 10.0;
    w.attack_range = 200.0;
    w.min_delay_between_shots = 30;
    w.max_delay_between_shots = 30;
    w
}

fn tank() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Tank");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.max_health = 400.0;
    t.initial_health = 400.0;
    t.geometry.major_radius = 6.0;
    t.geometry.minor_radius = 6.0;
    t.vision_range = 250.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t.ai_update = Some(0);
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(gun()));
    let mut c = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    c.auto_acquire_enemies = true;
    t.combat = c;
    t
}

/// A `SpySatellitePing`: `KindOf = NO_COLLIDE IMMOBILE UNATTACKABLE INERT`, an `ImmortalBody` (1 hp),
/// no AI. It is an enemy of the tank but must never be acquired.
fn ping() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Ping");
    t.kind_of = kind_of::NO_COLLIDE | kind_of::IMMOBILE | kind_of::UNATTACKABLE | kind_of::INERT;
    t.max_health = 10000.0;
    t.initial_health = 10000.0;
    t.geometry.major_radius = 1.0;
    t
}

/// An ordinary enemy vehicle to show the same scan does acquire a legal target.
fn junk() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Junk");
    t.kind_of = kind_of::VEHICLE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 5.0;
    t
}

fn world() -> GameLogic {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 1000);
    g.add_player(1, 1000);
    g.set_enemies(0, 1);
    g
}

/// The idle mood scan (`AIUpdateInterface::getNextMoodTarget`, AIUpdate.cpp:4700) runs
/// `AI::findClosestEnemy` -> `PartitionFilterPossibleToAttack` -> `WeaponSet::getAttackResult`, which
/// returns `ATTACKRESULT_NOT_POSSIBLE` for `KINDOF_UNATTACKABLE`.
///
/// Revert check: remove the `UNATTACKABLE`/`MASKED` test from `GameLogic::target_info` and the tank
/// enters `AttackObject` on the ping within a few frames.
#[test]
fn an_unattackable_ping_is_not_an_idle_target() {
    let mut g = world();
    let tank_t = Arc::new(tank());
    let ping_t = Arc::new(ping());
    let id = g.create_object(&tank_t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.create_object(&ping_t, Coord3D::new(150.0, 100.0, 0.0), 0.0, 1);
    g.run_frames(90);
    let o = g.find_object(id).unwrap();
    assert!(o.combat.attack.is_none(), "the ping is not a legal victim: {:?}", o.combat.attack);
    assert_eq!(o.ai.machine.current, AiStateType::Idle, "the tank stays Idle");
}

/// The control: the same idle scan acquires an ordinary enemy vehicle in range (`AI::CAN_ATTACK`
/// passes `PartitionFilterPossibleToAttack`).
#[test]
fn an_ordinary_enemy_is_still_an_idle_target() {
    let mut g = world();
    let tank_t = Arc::new(tank());
    let junk_t = Arc::new(junk());
    let id = g.create_object(&tank_t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let target = g.create_object(&junk_t, Coord3D::new(150.0, 100.0, 0.0), 0.0, 1);
    g.run_frames(90);
    let o = g.find_object(id).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::AttackObject, "the tank acquires the vehicle");
    assert_eq!(o.combat.attack.map(|a| a.target), Some(zr_sim::combat::AttackTarget::Object(target)));
}
