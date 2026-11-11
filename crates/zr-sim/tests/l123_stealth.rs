//! Lane 123: `StealthUpgrade` (`StealthUpgrade.cpp:51-70`), `allowedToStealth`'s moving/firing
//! conditions (`StealthUpdate.cpp:283, :340-402`) and the attack sub-machine's `IS_FIRING_WEAPON`
//! (`AIStates.cpp:5203, :5347`). Synthetic data only.

use std::sync::Arc;

use zr_sim::ai_states::AiStateType;
use zr_sim::behaviors::StealthTemplate;
use zr_sim::object::status;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams, SimpleTemplateStore};
use zr_sim::upgrade::{SimUpgradeModule, UpgradeEffect};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(400, 400, 0)
}

/// A stealth module with conditions of its own: innate, so `CAN_STEALTH` is set at construction
/// (`StealthUpdate.cpp:147-151`) and the module may stealth.
fn stealth(forbidden: u32, move_threshold_speed: f32) -> StealthTemplate {
    StealthTemplate {
        stealth_delay: 0,
        move_threshold_speed,
        forbidden_conditions: forbidden,
        required_status: 0,
        forbidden_status: 0,
        innate_stealth: true,
        granted_by_special_power: false,
        disguises_as_team: false,
        reveal_distance_from_target: 0.0,
        order_idle_enemies_to_attack_on_reveal: false,
        use_rider_stealth: false,
    }
}

/// The GLA camouflage modules' shape (`InnateStealth = No ;Requires upgrade first`): awake, off until
/// a `StealthUpgrade` sets `CAN_STEALTH`.
fn upgrade_gated_stealth() -> StealthTemplate {
    StealthTemplate { innate_stealth: false, ..stealth(0, 0.0) }
}

fn unit(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 200.0;
    t.initial_health = 200.0;
    t.vision_range = 200.0;
    t
}

/// Give a template a gun with the given range and an optional pre-attack delay.
fn armed(mut t: ObjectTemplate, range: f32, pre_attack_delay: u32) -> ObjectTemplate {
    let mut w = WeaponTemplate::new("Gun");
    w.primary_damage = 10.0;
    w.attack_range = range;
    w.min_delay_between_shots = 30;
    w.max_delay_between_shots = 30;
    w.pre_attack_delay = pre_attack_delay;
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(w));
    t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    t
}

fn wheels(mut t: ObjectTemplate) -> ObjectTemplate {
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    // An `AIUpdate` module, so the move/attack orders reach the unit (`getAI() != nullptr`).
    t.ai_update = Some(1);
    t
}

/// A `StealthUpgrade` module (`StealthUpgrade.cpp:40-70`): no module data of its own, just the mux.
fn stealth_upgrade() -> SimUpgradeModule {
    SimUpgradeModule {
        triggered_by: vec!["Upgrade_GLA_Camo".into()],
        conflicts_with: vec![],
        removes_upgrades: vec![],
        requires_all_triggers: false,
        effect: UpgradeEffect::Stealth,
    }
}

/// `StealthUpgrade::upgradeImplementation` (`StealthUpgrade.cpp:51-55`): the upgrade sets
/// `OBJECT_STATUS_CAN_STEALTH`; the awake stealth module notices and starts stealthing.
/// Revert check: drop `UpgradeEffect::Stealth`'s dispatch and the bit is never set.
#[test]
fn a_stealth_upgrade_sets_can_stealth_and_the_unit_stealths() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let mut t = unit("CamoTank");
    t.stealth = Some(upgrade_gated_stealth());
    t.upgrade_modules = vec![Arc::new(stealth_upgrade())];
    let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.run_frames(2);
    assert!(!g.is_stealthed(id), "without the upgrade the module cannot stealth");
    assert_eq!(g.find_object(id).unwrap().status_mask(g.frame()) & status::CAN_STEALTH, 0);

    g.give_upgrade(id, "Upgrade_GLA_Camo");
    assert_ne!(
        g.find_object(id).unwrap().status_mask(g.frame()) & status::CAN_STEALTH,
        0,
        "the upgrade sets OBJECT_STATUS_CAN_STEALTH"
    );
    g.run_frames(1);
    assert!(g.is_stealthed(id), "the awake module sees CAN_STEALTH and stealths");
}

/// `StealthUpgrade::upgradeImplementation`'s spawn branch (`StealthUpgrade.cpp:57-68`):
/// `SpawnBehavior::giveSlavesStealthUpgrade(TRUE)` (`SpawnBehavior.cpp:478-486`) sets `CAN_STEALTH`
/// on every slave. The spawner's `SpawnBehavior` makes the slave on its first update
/// (`spawn_create_spawn`), which sets `Object::master`. Revert check: drop the slave loop and the slave's
/// bit stays clear.
#[test]
fn a_stealth_upgrade_reaches_a_spawners_slaves() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let mut store = SimpleTemplateStore::default();
    let slave_template = store.add({
        let mut t = unit("Slave");
        t.stealth = Some(upgrade_gated_stealth());
        t
    });
    let slave_name = slave_template.name.clone();
    g.set_template_source(Arc::new(store));

    let mut spawner = unit("Spawner");
    spawner.kind_of |= kind_of::SPAWNS_ARE_THE_WEAPONS;
    spawner.kind_of |= kind_of::IMMOBILE;
    spawner.upgrade_modules = vec![Arc::new(stealth_upgrade())];
    spawner.spawn = Some(zr_sim::modules2::SpawnTemplate {
        spawn_number: 1,
        spawn_replace_delay: 9999,
        one_shot: true,
        spawn_template_names: vec![slave_name.clone()],
        spawned_require_spawner: true,
        initial_burst: 0,
        aggregate_health: false,
    });
    let s = g.create_object(&Arc::new(spawner), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.run_frames(3);
    // The spawner reaches its spawns through its own id list (`SpawnBehavior::m_spawnIDs`,
    // `giveSlavesStealthUpgrade`, SpawnBehavior.cpp:478-488); the synthetic slave has no `SlavedUpdate`, so it has no
    // `master` (only `m_slaver` holders get one, `:658-666`).
    let slave = g
        .objects()
        .find(|o| o.template.name == slave_template.name)
        .map(|o| o.id)
        .expect("SpawnBehavior made its slave");
    assert_eq!(g.find_object(slave).unwrap().status_mask(g.frame()) & status::CAN_STEALTH, 0);

    g.give_upgrade(s, "Upgrade_GLA_Camo");
    assert_ne!(g.find_object(s).unwrap().status_mask(g.frame()) & status::CAN_STEALTH, 0, "the spawner itself");
    assert_ne!(
        g.find_object(slave).unwrap().status_mask(g.frame()) & status::CAN_STEALTH,
        0,
        "giveSlavesStealthUpgrade reaches the slave"
    );
}

/// `STEALTH_NOT_WHILE_MOVING` reads `physics->getVelocityMagnitude() >
/// getStealthUpdateModuleData()->m_stealthSpeed` (`StealthUpdate.cpp:399-402`). Once the unit is
/// driving faster than its threshold it cannot stealth. This direction agrees with the old
/// `is_moving()` test; the threshold one below is the discriminating case.
#[test]
fn a_moving_stealth_unit_over_its_move_threshold_cannot_stealth() {
    use zr_sim::behaviors::stealth_forbidden;
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let mut t = wheels(unit("CamoTank"));
    t.physics = Some(zr_sim::PhysicsTemplate::default());
    t.stealth = Some(stealth(stealth_forbidden::MOVING, 0.0));
    let id = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.run_frames(2);
    assert!(g.is_stealthed(id), "at rest it stealths");
    g.submit(0, Command::MoveTo { ids: vec![id], pos: Coord3D::new(300.0, 100.0, 0.0) });
    g.run_frames(5);
    let o = g.find_object(id).unwrap();
    let speed = o.physics.as_ref().map_or(0.0, |p| p.velocity_magnitude());
    assert!(o.ai.is_moving() && speed > 0.0, "the unit is under way (speed {speed})");
    assert!(!g.is_stealthed(id), "physics velocity over MoveThresholdSpeed forbids the stealth");
}

/// The same rule with `MoveThresholdSpeed` above what the locomotor can reach: a unit whose AI is
/// moving but whose physics velocity stays under the threshold may stealth. Revert check: test
/// `ai.is_moving()` (the old code) and the unit is unstealthed as soon as it drives.
#[test]
fn a_moving_stealth_unit_below_its_move_threshold_keeps_its_stealth() {
    use zr_sim::behaviors::stealth_forbidden;
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let mut t = wheels(unit("CamoTank"));
    t.physics = Some(zr_sim::PhysicsTemplate::default());
    // The locomotor's top speed is 30 units/s = 1 unit/frame, far under 100.
    t.stealth = Some(stealth(stealth_forbidden::MOVING, 100.0));
    let id = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.run_frames(2);
    assert!(g.is_stealthed(id));
    g.submit(0, Command::MoveTo { ids: vec![id], pos: Coord3D::new(300.0, 100.0, 0.0) });
    g.run_frames(6);
    let o = g.find_object(id).unwrap();
    let speed = o.physics.as_ref().map_or(0.0, |p| p.velocity_magnitude());
    assert!(o.ai.is_moving() && speed > 0.0 && speed < 100.0, "moving, under the threshold (speed {speed})");
    assert!(g.is_stealthed(id), "below MoveThresholdSpeed the stealth stays");
}

/// `STEALTH_NOT_WHILE_ATTACKING` tests `OBJECT_STATUS_IS_FIRING_WEAPON` (`StealthUpdate.cpp:283`),
/// not the attack order: an attack that is still closing in has not fired, so the unit may stealth.
/// Revert check: test `combat.attack.is_some()` and the unit is unstealthed from the first attack frame.
#[test]
fn a_stealth_unit_may_stealth_while_its_attack_closes_in() {
    use zr_sim::behaviors::stealth_forbidden;
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    g.reveal_map_permanently_for_player(0);
    let mut t = wheels(armed(unit("CamoTank"), 100.0, 0));
    t.stealth = Some(stealth(stealth_forbidden::ATTACKING, 0.0));
    let id = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    // Out of weapon range, and far enough that a few frames cannot reach it.
    let far = g.create_object(&Arc::new(unit("Target")), Coord3D::new(380.0, 100.0, 0.0), 0.0, 1);
    g.run_frames(1);
    g.submit(0, Command::Attack { ids: vec![id], target: far });
    g.run_frames(3);
    let o = g.find_object(id).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::AttackObject, "the attack is still running");
    assert_eq!(o.status_mask(g.frame()) & status::IS_FIRING_WEAPON, 0, "it has not fired");
    assert!(g.is_stealthed(id), "an attack order alone does not block the stealth");
}

/// `STEALTH_NOT_WHILE_FIRING_WEAPON` (`StealthUpdate.cpp:340-397`): the all-three form fails while
/// `OBJECT_STATUS_IS_FIRING_WEAPON` is set. The weapon's pre-attack keeps the fire state (and the
/// bit) alive for several frames. Revert check: drop the firing test and the unit stealths with the
/// bit set.
#[test]
fn the_firing_status_blocks_a_firing_stealth_unit() {
    use zr_sim::behaviors::stealth_forbidden;
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    g.reveal_map_permanently_for_player(0);
    let mut t = wheels(armed(unit("CamoTank"), 100.0, 20));
    t.stealth = Some(stealth(stealth_forbidden::FIRING_WEAPON, 0.0));
    let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let victim = g.create_object(&Arc::new(unit("Victim")), Coord3D::new(250.0, 200.0, 0.0), 0.0, 1);
    g.run_frames(2);
    assert!(g.is_stealthed(id), "not firing, so it stealths");

    g.submit(0, Command::Attack { ids: vec![id], target: victim });
    g.run_frames(3);
    let o = g.find_object(id).unwrap();
    assert_ne!(o.status_mask(g.frame()) & status::IS_FIRING_WEAPON, 0, "the fire state is entered (pre-attack)");
    assert_eq!(g.find_object(victim).unwrap().health(), 200.0, "the pre-attack has not fired yet");
    assert!(!g.is_stealthed(id), "the firing bit forbids the stealth");
}

/// `AIAttackFireWeaponState::onEnter` sets `OBJECT_STATUS_IS_FIRING_WEAPON` (`AIStates.cpp:5203`) and
/// its `onExit` clears it (`:5347`); `AIAttackState::onExit` clears it too (`:5727`). The port had the
/// bit defined but nothing set it. Revert check: drop the set in the aim->fire transition and the
/// first assert fails.
#[test]
fn the_attack_sets_the_firing_status_until_it_fires() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    g.reveal_map_permanently_for_player(0);
    let a = g.create_object(&Arc::new(wheels(armed(unit("Attacker"), 100.0, 0))), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let v = g.create_object(&Arc::new(unit("Victim")), Coord3D::new(250.0, 200.0, 0.0), 0.0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: v });
    g.run_frames(2);
    let o = g.find_object(a).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::AttackObject);
    assert_ne!(o.status_mask(g.frame()) & status::IS_FIRING_WEAPON, 0, "the fire state is entered: the bit is on");
    assert_eq!(g.find_object(v).unwrap().health(), 200.0, "it has not fired yet");

    g.run_frames(1);
    assert_eq!(g.find_object(v).unwrap().health(), 190.0, "the shot lands");
    assert_eq!(
        g.find_object(a).unwrap().status_mask(g.frame()) & status::IS_FIRING_WEAPON,
        0,
        "the fire state's exit clears it"
    );

    // `AIAttackState::onExit` (`AIStates.cpp:5727`) clears it too, even mid-cycle.
    g.ai_idle(a, zr_sim::ai_states::CommandSource::Player);
    assert_eq!(g.find_object(a).unwrap().status_mask(g.frame()) & status::IS_FIRING_WEAPON, 0, "an idle order clears it");
}
