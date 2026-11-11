//! Weapons, armor, damage and the attack command, on synthetic data only.

use std::sync::Arc;
use zr_sim::armor::ArmorTemplate;
use zr_sim::damage::DamageType;
use zr_sim::template::{kind_of, Geometry, GeometryShape, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{affects, CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{AttackTarget, Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(100, 100, 0)
}

fn armor_ap_half() -> Arc<ArmorTemplate> {
    let mut a = ArmorTemplate::new("TankArmor");
    a.coefficients[DamageType::ArmorPiercing.index()] = 0.5;
    Arc::new(a)
}

/// 40 armor-piercing damage, 30 frames between shots, instant hit, range 150.
fn cannon() -> WeaponTemplate {
    let mut w = WeaponTemplate::new("Cannon");
    w.primary_damage = 40.0;
    w.damage_type = DamageType::ArmorPiercing;
    w.attack_range = 150.0;
    w.min_delay_between_shots = 30;
    w.max_delay_between_shots = 30;
    w
}

fn tank_with(name: &str, weapon: Option<WeaponTemplate>) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.vision_range = 200.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t.combat = CombatTemplate {
        armor: Some(armor_ap_half()),
        weapon_set: weapon.map(|w| {
            let mut s = WeaponSetTemplate::default();
            s.weapons[0] = Some(Arc::new(w));
            s
        }),
        ..CombatTemplate::default()
    };
    t
}

/// A static structure footprint (`classifyObjectFootprint` makes it a pathfinder obstacle).
fn building() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Wall");
    t.kind_of = kind_of::STRUCTURE;
    t.geometry = Geometry { shape: GeometryShape::Box, major_radius: 40.0, minor_radius: 40.0, height: 20.0, is_small: false };
    t
}

fn duel_world(seed: u32) -> (GameLogic, ObjectId, ObjectId) {
    let mut g = GameLogic::new(flat(), seed);
    let a = Arc::new(tank_with("A", Some(cannon())));
    let b = Arc::new(tank_with("B", None));
    let ia = g.create_object(&a, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let ib = g.create_object(&b, Coord3D::new(300.0, 200.0, 0.0), 0.0, 1);
    (g, ia, ib)
}

#[test]
fn tank_kills_tank_in_expected_shots_and_frames() {
    let (mut g, a, b) = duel_world(1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    // 40 AP damage * 0.5 armor = 20 per shot, 100 health => 5 shots.
    // The unit was created before the first update, so its AI wakes at frame 1 (`setWakeFrame(NONE)` at
    // creation, AIUpdate.cpp:299; `m_frame++` at the end of `GameLogic::update`, GameLogic.cpp:3929). The
    // aim is frame 1 and the fire frame 2 (aim -> fire costs a frame), then every 30 frames: the shots land
    // on the updates of frames 2, 32, 62, 92 and 122. `run_frames(n)` has run the first n updates.
    g.run_frames(3);
    assert_eq!(g.find_object(b).unwrap().health(), 80.0);
    g.run_frames(30);
    assert_eq!(g.find_object(b).unwrap().health(), 60.0);
    g.run_frames(88); // through the update of frame 120
    assert_eq!(g.find_object(b).unwrap().health(), 20.0);
    g.run_frames(1); // frame 121: no shot yet
    assert_eq!(g.find_object(b).unwrap().health(), 20.0);
    assert_eq!(g.frame(), 122);
    g.run_frames(1); // frame 122: fifth shot
    assert!(g.find_object(b).is_none(), "target must be dead");
    assert_eq!(g.frame(), 123);
    // The attacker stands down once the victim is dead.
    g.run_frames(5);
    assert!(g.find_object(a).unwrap().combat.attack.is_none());
}

#[test]
fn armor_scales_by_damage_type_and_unresistable_bypasses() {
    let mut g = GameLogic::new(flat(), 1);
    let t = Arc::new(tank_with("T", None));
    let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    use zr_sim::body::DamageInfo;
    let hit = |kind| DamageInfo { source: ObjectId::INVALID, amount: 20.0, kind };
    assert_eq!(g.damage_object(id, hit(DamageType::ArmorPiercing)), 10.0);
    assert_eq!(g.damage_object(id, hit(DamageType::SmallArms)), 20.0);
    assert_eq!(g.damage_object(id, hit(DamageType::Unresistable)), 20.0);
    assert_eq!(g.find_object(id).unwrap().health(), 50.0);
}

#[test]
fn radius_damage_hits_multiple_with_primary_and_secondary() {
    let mut g = GameLogic::new(flat(), 1);
    let mut w = WeaponTemplate::new("Boom");
    w.primary_damage = 50.0;
    w.primary_damage_radius = 20.0;
    w.secondary_damage = 10.0;
    w.secondary_damage_radius = 60.0;
    w.attack_range = 300.0;
    w.min_delay_between_shots = 1000; // one blast only
    w.max_delay_between_shots = 1000;
    w.damage_type = DamageType::Explosion;
    let shooter = g.create_object(&Arc::new(tank_with("S", Some(w))), Coord3D::new(300.0, 400.0, 0.0), 0.0, 0);
    let dummy = Arc::new({
        let mut d = tank_with("D", None);
        d.combat.armor = None;
        d.geometry.major_radius = 2.0;
        d
    });
    // centre of the blast at (400, 400)
    let near = g.create_object(&dummy, Coord3D::new(405.0, 400.0, 0.0), 0.0, 1); // within primary
    let mid = g.create_object(&dummy, Coord3D::new(400.0, 440.0, 0.0), 0.0, 1); // secondary (gap 38)
    let far = g.create_object(&dummy, Coord3D::new(400.0, 520.0, 0.0), 0.0, 1); // outside
    let friend = g.create_object(&dummy, Coord3D::new(395.0, 400.0, 0.0), 0.0, 0); // default affects allies too
    g.submit(0, Command::AttackGround { ids: vec![shooter], pos: Coord3D::new(400.0, 400.0, 0.0) });
    g.run_frames(4);
    let hp = |id| g.find_object(id).unwrap().health();
    assert_eq!(hp(near), 50.0);
    assert_eq!(hp(mid), 90.0);
    assert_eq!(hp(far), 100.0);
    assert_eq!(hp(friend), 50.0);
    assert_eq!(hp(shooter), 100.0, "the shooter is out of the blast");
}

#[test]
fn clip_reload_timing_in_frames() {
    let mut w = cannon();
    w.primary_damage = 1.0;
    w.min_delay_between_shots = 5;
    w.max_delay_between_shots = 5;
    w.clip_size = 3;
    w.clip_reload_time = 40;
    let mut g = GameLogic::new(flat(), 1);
    let a = g.create_object(&Arc::new(tank_with("A", Some(w))), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let mut dummy = tank_with("B", None);
    dummy.max_health = 1000.0;
    dummy.initial_health = 1000.0;
    let b = g.create_object(&Arc::new(dummy), Coord3D::new(300.0, 200.0, 0.0), 0.0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    let mut shots = Vec::new();
    let mut last = g.find_object(a).unwrap().combat.weapons.weapon(0).unwrap().ammo_in_clip();
    for _ in 0..80 {
        g.update();
        let ammo = g.find_object(a).unwrap().combat.weapons.weapon(0).unwrap().ammo_in_clip();
        if ammo != last && ammo < last || (ammo == 3 && last == 1) {
            shots.push(g.frame() - 1);
        }
        last = ammo;
    }
    // 5-frame delay quantised by the aim/fire alternation => every 6 frames; the 3rd shot empties
    // the clip and the 40-frame reload runs from that frame. The first AI update is frame 1 for a unit
    // created before the first update (AIUpdate.cpp:299, GameLogic.cpp:3929), so the first aim is
    // frame 1 and the first fire frame 2; every later index shifts by the same one frame.
    assert_eq!(shots, vec![2, 8, 14, 54, 60, 66]);
}

#[test]
fn unit_chases_into_range_then_fires() {
    let mut g = GameLogic::new(flat(), 1);
    let a = g.create_object(&Arc::new(tank_with("A", Some(cannon()))), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(tank_with("B", None)), Coord3D::new(600.0, 100.0, 0.0), 0.0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    g.run_frames(30);
    assert_eq!(g.find_object(b).unwrap().health(), 100.0, "still out of range");
    assert!(g.find_object(a).unwrap().position.x > 100.0);
    let mut killed_at = None;
    for _ in 0..1200 {
        g.update();
        if g.find_object(b).is_none() {
            killed_at = Some(g.frame());
            break;
        }
    }
    assert!(killed_at.is_some(), "target should die");
    let pa = g.find_object(a).unwrap().position;
    // attacker stopped within range (150 + radii) of where the victim was
    assert!(pa.x > 100.0 && 600.0 - pa.x < 150.0 + 16.0, "attacker x = {}", pa.x);
}

#[test]
fn chase_routes_around_a_structure() {
    let mut g = GameLogic::new(flat(), 1);
    let a = g.create_object(&Arc::new(tank_with("A", Some(cannon()))), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
    // 80x80 static footprint directly between the attacker and the victim.
    g.create_object(&Arc::new(building()), Coord3D::new(400.0, 300.0, 0.0), 0.0, 1);
    let b = g.create_object(&Arc::new(tank_with("B", None)), Coord3D::new(700.0, 300.0, 0.0), 0.0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    let mut max_deviation = 0.0f32;
    let mut killed = false;
    for _ in 0..1500 {
        g.update();
        if let Some(o) = g.find_object(a) {
            max_deviation = max_deviation.max((o.position.y - 300.0).abs());
        }
        if g.find_object(b).is_none() {
            killed = true;
            break;
        }
    }
    assert!(killed, "the attacker must path around the wall and reach the victim");
    assert!(max_deviation > 40.0, "the chase must bend around the structure (max lateral {max_deviation})");
}

#[test]
fn chase_repaths_when_target_moves() {
    let mut g = GameLogic::new(flat(), 1);
    let a = g.create_object(&Arc::new(tank_with("A", Some(cannon()))), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(tank_with("B", None)), Coord3D::new(600.0, 100.0, 0.0), 0.0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    g.run_frames(20);
    let goal_before = g.find_object(a).unwrap().ai.goal().expect("chasing");
    // Send the victim far to the south. The attacker's old path is now stale; after
    // MIN_RECOMPUTE_TIME and the moved test it must re-path toward the victim.
    g.submit(1, Command::MoveTo { ids: vec![b], pos: Coord3D::new(600.0, 900.0, 0.0) });
    g.run_frames(160);
    let victim = g.find_object(b).expect("victim alive").position;
    let goal_after = g.find_object(a).expect("attacker alive").ai.goal().expect("still chasing");
    assert!(goal_after.dist_2d(&victim) < 170.0, "goal {goal_after:?} should track the victim {victim:?}");
    assert!(goal_after.y - goal_before.y > 40.0, "goal must move south: {goal_before:?} -> {goal_after:?}");
}

#[test]
fn radius_damage_respects_affects_mask_with_allies() {
    let mut g = GameLogic::new(flat(), 1);
    let mut w = WeaponTemplate::new("EnemyOnlyBlast");
    w.primary_damage = 50.0;
    w.primary_damage_radius = 40.0;
    w.attack_range = 300.0;
    w.min_delay_between_shots = 1000; // one blast only
    w.max_delay_between_shots = 1000;
    w.damage_type = DamageType::Explosion;
    w.affects = affects::ENEMIES; // the affects mask excludes allies and neutrals
    let shooter = g.create_object(&Arc::new(tank_with("S", Some(w))), Coord3D::new(300.0, 400.0, 0.0), 0.0, 0);
    let dummy = Arc::new({
        let mut d = tank_with("D", None);
        d.combat.armor = None;
        d.geometry.major_radius = 2.0;
        d
    });
    let ally = g.create_object(&dummy, Coord3D::new(400.0, 400.0, 0.0), 0.0, 2);
    let enemy = g.create_object(&dummy, Coord3D::new(410.0, 400.0, 0.0), 0.0, 1);
    g.set_allied(0, 2);
    g.set_enemies(0, 1);
    g.submit(0, Command::AttackGround { ids: vec![shooter], pos: Coord3D::new(405.0, 400.0, 0.0) });
    g.run_frames(4);
    assert_eq!(g.find_object(ally).unwrap().health(), 100.0, "an allies-only-excluded blast spares the ally");
    assert_eq!(g.find_object(enemy).unwrap().health(), 50.0, "the enemy is inside the blast");
}

#[test]
fn projectile_weapon_lands_after_flight_time() {
    let mut w = cannon();
    w.has_projectile = true;
    w.weapon_speed = 5.0; // per frame
    let mut g = GameLogic::new(flat(), 1);
    let a = g.create_object(&Arc::new(tank_with("A", Some(w))), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(tank_with("B", None)), Coord3D::new(300.0, 200.0, 0.0), 0.0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    g.run_frames(3);
    assert_eq!(g.combat_state().projectile_count(), 1);
    assert_eq!(g.find_object(b).unwrap().health(), 100.0);
    // 100 units apart, 8 victim radius: ~18 frames of flight.
    g.run_frames(25);
    assert_eq!(g.find_object(b).unwrap().health(), 80.0);
    assert_eq!(g.combat_state().projectile_count(), 0);
}

#[test]
fn idle_unit_auto_acquires_nearby_enemy_but_not_allies() {
    let mut g = GameLogic::new(flat(), 1);
    let mut at = tank_with("A", Some(cannon()));
    at.combat.auto_acquire_enemies = true;
    at.combat.mood_attack_check_rate = 10;
    let a = g.create_object(&Arc::new(at), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let ally = g.create_object(&Arc::new(tank_with("Ally", None)), Coord3D::new(250.0, 200.0, 0.0), 0.0, 2);
    g.set_allied(0, 2);
    g.run_frames(40);
    assert_eq!(g.find_object(ally).unwrap().health(), 100.0);
    assert!(g.find_object(a).unwrap().combat.attack.is_none());
    let enemy = g.create_object(&Arc::new(tank_with("Enemy", None)), Coord3D::new(300.0, 200.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.run_frames(30);
    assert!(g.find_object(enemy).unwrap().health() < 100.0);
    assert_eq!(g.find_object(a).unwrap().combat.attack.map(|o| o.target), Some(AttackTarget::Object(enemy)));
}

#[test]
fn cannot_attack_own_units() {
    let mut g = GameLogic::new(flat(), 1);
    let a = g.create_object(&Arc::new(tank_with("A", Some(cannon()))), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let friend = g.create_object(&Arc::new(tank_with("F", None)), Coord3D::new(250.0, 200.0, 0.0), 0.0, 0);
    g.submit(0, Command::Attack { ids: vec![a], target: friend });
    g.run_frames(10);
    assert_eq!(g.find_object(friend).unwrap().health(), 100.0);
}

#[test]
fn move_order_cancels_attack() {
    let (mut g, a, b) = duel_world(1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    g.run_frames(5);
    g.submit(0, Command::MoveTo { ids: vec![a], pos: Coord3D::new(100.0, 200.0, 0.0) });
    g.run_frames(200);
    assert!(g.find_object(a).unwrap().combat.attack.is_none());
    assert!(g.find_object(b).unwrap().health() >= 80.0);
}

fn duel_crc_trace(seed: u32) -> Vec<u32> {
    let (mut g, a, b) = duel_world(seed);
    // scatter + random shot delay exercise the RNG
    let mut w = cannon();
    w.scatter_radius = 5.0;
    w.primary_damage_radius = 12.0;
    w.min_delay_between_shots = 20;
    w.max_delay_between_shots = 40;
    let c = g.create_object(&Arc::new(tank_with("C", Some(w))), Coord3D::new(200.0, 260.0, 0.0), 0.0, 0);
    let mut out = Vec::new();
    g.submit(0, Command::Attack { ids: vec![a, c], target: b });
    for _ in 0..200 {
        g.update();
        out.push(g.crc());
    }
    out
}

#[test]
fn two_instances_agree_every_frame() {
    let x = duel_crc_trace(7);
    let y = duel_crc_trace(7);
    assert_eq!(x, y);
    assert!(x.windows(2).any(|w| w[0] != w[1]));
    assert_ne!(x, duel_crc_trace(8), "different seeds must diverge once the RNG is used");
}

#[test]
fn ai_hunt_acquires_distant_enemy_building() {
    // An AI-controlled unit in `AIHuntState` ignores fog and targets enemy buildings anywhere
    // (`PartitionFilterRejectBuildings` allows them for computer players).
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 1000);
    g.add_player(1, 1000);
    g.set_enemies(0, 1);
    g.add_ai_player(0, zr_sim::SkirmishAiConfig::default());
    let mut tt = tank_with("A", Some(cannon()));
    tt.combat.auto_acquire_enemies = true;
    let tank = Arc::new(tt);
    let mut bt = building();
    bt.max_health = 100.0;
    bt.initial_health = 100.0;
    let wall = Arc::new(bt);
    let a = g.create_object(&tank, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let _b = g.create_object(&wall, Coord3D::new(800.0, 200.0, 0.0), 0.0, 1);
    g.command_hunt(0, &[a]);
    // `AIHuntState::onEnter` picks a random first scan offset in `0..ENEMY_SCAN_RATE`, so give it
    // more than one scan interval.
    g.run_frames(40);
    let o = g.find_object(a).unwrap();
    assert!(o.combat.attack.is_some(), "the hunting AI acquired the distant enemy building");
}

/// A `MP_COUNT_FOR_VICTORY` structure (what `VictoryConditions` counts).
fn victory_building() -> ObjectTemplate {
    let mut t = building();
    t.kind_of |= kind_of::MP_COUNT_FOR_VICTORY;
    t
}

#[test]
fn victory_nobuildings_ends_when_a_player_loses_all_structures() {
    // Skirmish uses `VICTORY_NOBUILDINGS` (GameLogic.cpp:1702): a player is out once they have no
    // live `KINDOF_MP_COUNT_FOR_VICTORY` buildings, even if units survive.
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 1000);
    g.add_player(1, 1000);
    g.set_victory_players([0, 1]);
    let mut bt = victory_building();
    bt.max_health = 100.0;
    g.create_object(&Arc::new(bt), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    // Player 1 has a unit but no victory building -> defeated.
    let tank = Arc::new(tank_with("B", None));
    g.create_object(&tank, Coord3D::new(400.0, 200.0, 0.0), 0.0, 1);
    let end = g.check_for_game_end();
    assert_eq!(end.winner, Some(0));
    assert_eq!(end.defeated, vec![1]);
}

#[test]
fn victory_update_kills_the_defeated_players_units() {
    // `VictoryConditions::update` -> `Player::killPlayer`: once a player has no
    // `MP_COUNT_FOR_VICTORY` buildings, every remaining object is killed (Object.cpp:2010).
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 1000);
    g.add_player(1, 1000);
    g.set_victory_players([0, 1]);
    let mut bt = victory_building();
    bt.max_health = 100.0;
    g.create_object(&Arc::new(bt), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let mut tank = tank_with("B", None);
    // A real unit template carries a `Destroy` die module; without one the body dies in place.
    tank.die = vec![zr_sim::die::DieModuleTemplate {
        mux: zr_sim::die::DieMux { death_types: u32::MAX, veterancy_levels: u32::MAX, exempt_status: 0, required_status: 0 },
        kind: zr_sim::die::DieTemplate::Destroy,
    }];
    let doomed = g.create_object(&Arc::new(tank), Coord3D::new(400.0, 200.0, 0.0), 0.0, 1);
    assert!(g.find_object(doomed).is_some());
    // Run a frame with no player-1 victory building: the elimination runs at end of frame.
    g.update();
    assert!(g.find_object(doomed).is_none(), "the defeated player's unit was killed");
}

/// A structure without `MP_COUNT_FOR_VICTORY` does not keep a player alive.
#[test]
fn victory_ignores_structures_without_the_count_flag() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 1000);
    g.add_player(1, 1000);
    g.set_victory_players([0, 1]);
    let mut bt = building(); // no MP_COUNT_FOR_VICTORY
    bt.max_health = 100.0;
    g.create_object(&Arc::new(bt), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    assert_eq!(g.check_for_game_end().winner, None);
    assert!(!g.player_alive(0));
}
