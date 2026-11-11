//! Possession (docs/POSSESSION.md) on synthetic data only.
#![cfg(feature = "possession")]

use std::sync::Arc;
use zr_sim::armor::ArmorTemplate;
use zr_sim::body::DamageInfo;
use zr_sim::command::buttons;
use zr_sim::damage::DamageType;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams, ZBehavior};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{AttackTarget, Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, PossessInput, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(100, 100, 0)
}

fn armor_ap_half() -> Arc<ArmorTemplate> {
    let mut a = ArmorTemplate::new("TankArmor");
    a.coefficients[DamageType::ArmorPiercing.index()] = 0.5;
    Arc::new(a)
}

/// 40 AP damage, 30 frames between shots, instant hit (hitscan), range 150.
fn cannon() -> WeaponTemplate {
    let mut w = WeaponTemplate::new("Cannon");
    w.primary_damage = 40.0;
    w.damage_type = DamageType::ArmorPiercing;
    w.attack_range = 150.0;
    w.min_delay_between_shots = 30;
    w.max_delay_between_shots = 30;
    w
}

const TURRET_RATE: f32 = 0.05;

fn tank(name: &str, weapon: Option<WeaponTemplate>, turret: bool) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.geometry.height = 10.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.vision_range = 200.0;
    // 30 units/s = 1 per frame; accel 60/s^2; braking 120/s^2; 120 deg/s.
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t.combat = CombatTemplate {
        armor: Some(armor_ap_half()),
        weapon_set: weapon.map(|w| {
            let mut s = WeaponSetTemplate::default();
            s.weapons[0] = Some(Arc::new(w));
            s
        }),
        turret_turn_rate: turret.then_some(TURRET_RATE),
        ..CombatTemplate::default()
    };
    Arc::new(t)
}

fn input(mx: f32, my: f32, aim: Coord3D, fire: bool) -> Command {
    Command::PossessInput(PossessInput {
        move_x: (mx * 32767.0) as i16,
        move_y: (my * 32767.0) as i16,
        aim,
        buttons: if fire { buttons::FIRE } else { 0 },
    })
}

fn world() -> (GameLogic, ObjectId) {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&tank("A", Some(cannon()), true), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    (g, id)
}

#[test]
fn possess_validation() {
    let mut g = GameLogic::new(flat(), 1);
    let t = tank("T", Some(cannon()), true);
    let mut st = ObjectTemplate::new("Bld");
    st.kind_of = kind_of::STRUCTURE;
    let st = Arc::new(st);
    let mine = g.create_object(&t, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let mine2 = g.create_object(&t, Coord3D::new(250.0, 200.0, 0.0), 0.0, 0);
    let theirs = g.create_object(&t, Coord3D::new(300.0, 200.0, 0.0), 0.0, 1);
    let bld = g.create_object(&st, Coord3D::new(400.0, 200.0, 0.0), 0.0, 0);

    // Wrong owner, structure, unknown id: ignored.
    for id in [theirs, bld, ObjectId(999)] {
        g.submit(0, Command::Possess { id });
        g.update();
        assert_eq!(g.possessed_unit(0), None, "{id:?}");
        assert!(!g.find_object(id).is_some_and(|o| o.ai.suspended));
    }
    // Input and release without possession are no-ops.
    g.submit(0, input(1.0, 0.0, Coord3D::ZERO, true));
    g.submit(0, Command::Release);
    g.update();
    assert_eq!(g.possessed_unit(0), None);

    g.submit(0, Command::Possess { id: mine });
    g.update();
    assert_eq!(g.possessed_unit(0), Some(mine));
    assert!(g.find_object(mine).unwrap().ai.suspended);
    // One unit per player: possessing another switches and un-suspends the first.
    g.submit(0, Command::Possess { id: mine2 });
    g.update();
    assert_eq!(g.possessed_unit(0), Some(mine2));
    assert!(!g.find_object(mine).unwrap().ai.suspended);
    assert!(g.find_object(mine2).unwrap().ai.suspended);
    // Another player cannot take it, and orders to a possessed unit are ignored.
    g.submit(1, Command::Possess { id: mine2 });
    g.submit(0, Command::MoveTo { ids: vec![mine2], pos: Coord3D::new(500.0, 500.0, 0.0) });
    g.run_frames(3);
    assert_eq!(g.possessed_unit(1), None);
    assert!(!g.find_object(mine2).unwrap().ai.is_moving());
    g.submit(0, Command::Release);
    g.update();
    assert_eq!(g.possessed_unit(0), None);
    assert!(!g.find_object(mine2).unwrap().ai.suspended);
}

#[test]
fn drives_along_world_x_at_locomotor_speed() {
    let (mut g, id) = world();
    g.submit(0, Command::Possess { id });
    let max = 1.0f32;
    let mut last_x = 200.0;
    for f in 0..60 {
        g.submit(0, input(1.0, 0.0, Coord3D::new(400.0, 200.0, 0.0), false));
        g.update();
        let o = g.find_object(id).unwrap();
        assert!(o.ai.speed <= max + 1e-4, "frame {f}: speed {}", o.ai.speed);
        assert!(o.position.x >= last_x);
        assert!((o.position.y - 200.0).abs() < 0.01);
        last_x = o.position.x;
    }
    let o = g.find_object(id).unwrap();
    assert!((o.ai.speed - max).abs() < 1e-3, "reaches top speed: {}", o.ai.speed);
    // Accelerating from rest: strictly less than 60 * max.
    assert!(o.position.x > 200.0 + 40.0 && o.position.x < 200.0 + 60.0);
}

#[test]
fn half_stick_halves_top_speed_and_diagonal_input_steers() {
    let (mut g, id) = world();
    g.submit(0, Command::Possess { id });
    for _ in 0..80 {
        g.submit(0, input(0.5, 0.0, Coord3D::ZERO, false));
        g.update();
    }
    assert!((g.find_object(id).unwrap().ai.speed - 0.5).abs() < 1e-3);
    // Now steer to +y: the hull turns at its own rate (120 deg/s = 0.0698 rad/frame).
    let a0 = g.find_object(id).unwrap().angle;
    g.submit(0, input(0.0, 1.0, Coord3D::ZERO, false));
    g.update();
    let a1 = g.find_object(id).unwrap().angle;
    assert!((a1 - a0).abs() <= 120.0f32.to_radians() / 30.0 + 1e-5 && a1 > a0);
}

#[test]
fn releasing_the_stick_decelerates_to_a_stop() {
    let (mut g, id) = world();
    g.submit(0, Command::Possess { id });
    for _ in 0..40 {
        g.submit(0, input(1.0, 0.0, Coord3D::ZERO, false));
        g.update();
    }
    let mut prev = g.find_object(id).unwrap().ai.speed;
    assert!(prev > 0.9);
    let mut frames = 0;
    while prev > 0.0 {
        g.submit(0, input(0.0, 0.0, Coord3D::ZERO, false));
        g.update();
        let s = g.find_object(id).unwrap().ai.speed;
        assert!(s < prev, "monotonic braking");
        prev = s;
        frames += 1;
        assert!(frames < 30);
    }
    let x = g.find_object(id).unwrap().position.x;
    g.run_frames(5);
    assert_eq!(g.find_object(id).unwrap().position.x, x, "stays stopped");
}

#[test]
fn turret_slews_toward_aim_at_its_rate() {
    let (mut g, id) = world();
    g.submit(0, Command::Possess { id });
    let aim = Coord3D::new(200.0, 400.0, 0.0); // due +y: pi/2
    let mut prev = 0.0f32;
    for f in 1..=40 {
        g.submit(0, input(0.0, 0.0, aim, false));
        g.update();
        let o = g.find_object(id).unwrap();
        let t = o.combat.turret_angle;
        assert!(t - prev <= TURRET_RATE + 1e-6, "frame {f}");
        assert_eq!(o.angle, 0.0, "hull does not turn");
        if f == 1 {
            assert!((t - TURRET_RATE).abs() < 1e-6);
        }
        prev = t;
    }
    // 1.5708 / 0.05 = 31.4 frames, so it has arrived by 40.
    assert!((prev - std::f32::consts::FRAC_PI_2).abs() < 1e-3);
}

#[test]
fn fire_kills_in_range_on_weapon_timing_and_not_out_of_range() {
    let mut g = GameLogic::new(flat(), 1);
    let a = g.create_object(&tank("A", Some(cannon()), true), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let near = g.create_object(&tank("B", None, false), Coord3D::new(300.0, 200.0, 0.0), 0.0, 1);
    let far = g.create_object(&tank("C", None, false), Coord3D::new(200.0, 600.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Possess { id: a });

    // Out of range target: turret aligns, no shot is taken.
    let far_pos = Coord3D::new(200.0, 600.0, 0.0);
    for _ in 0..60 {
        g.submit(0, input(0.0, 0.0, far_pos, true));
        g.update();
    }
    assert_eq!(g.find_object(far).unwrap().health(), 100.0);
    assert_eq!(g.find_object(a).unwrap().combat.weapons.weapon(0).unwrap().last_fire_frame(), 0);

    // In range: 20 damage per shot, 30 frames apart, dead on the fifth.
    let near_pos = Coord3D::new(300.0, 200.0, 0.0);
    let mut shots_at = Vec::new();
    let mut last = 100.0;
    for _ in 0..400 {
        g.submit(0, input(0.0, 0.0, near_pos, true));
        g.update();
        let h = g.find_object(near).map_or(0.0, |o| o.health());
        if h < last {
            shots_at.push(g.frame());
            last = h;
        }
        if g.find_object(near).is_none() {
            break;
        }
    }
    assert!(g.find_object(near).is_none(), "target killed");
    assert_eq!(shots_at.len(), 5);
    for w in shots_at.windows(2) {
        assert_eq!(w[1] - w[0], 30);
    }
}

#[test]
fn projectile_weapon_fires_at_the_aim_point() {
    let mut w = cannon();
    w.has_projectile = true;
    w.weapon_speed = 20.0;
    w.primary_damage_radius = 10.0;
    w.secondary_damage_radius = 10.0;
    w.secondary_damage = 40.0;
    let mut g = GameLogic::new(flat(), 1);
    let a = g.create_object(&tank("A", Some(w), true), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let b = g.create_object(&tank("B", None, false), Coord3D::new(300.0, 200.0, 0.0), 0.0, 1);
    g.submit(0, Command::Possess { id: a });
    for _ in 0..10 {
        g.submit(0, input(0.0, 0.0, Coord3D::new(300.0, 200.0, 0.0), true));
        g.update();
    }
    assert!(g.find_object(b).unwrap().health() < 100.0, "shell landed on the aim point");
}

#[test]
fn hitscan_picks_the_nearer_enemy_and_ignores_allies() {
    let mut g = GameLogic::new(flat(), 1);
    let a = g.create_object(&tank("A", Some(cannon()), true), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let t = tank("T", None, false);
    // Created far-first so id order opposes distance order.
    let far = g.create_object(&t, Coord3D::new(330.0, 200.0, 0.0), 0.0, 1);
    let near = g.create_object(&t, Coord3D::new(280.0, 200.0, 0.0), 0.0, 1);
    let ally = g.create_object(&t, Coord3D::new(240.0, 200.0, 0.0), 0.0, 0);
    g.set_enemies(0, 1);
    let aim = Coord3D::new(330.0, 200.0, 0.0);
    assert_eq!(g.hitscan_target(a, &aim, 150.0), AttackTarget::Object(near));

    g.submit(0, Command::Possess { id: a });
    for _ in 0..3 {
        g.submit(0, input(0.0, 0.0, aim, true));
        g.update();
    }
    assert_eq!(g.find_object(near).unwrap().health(), 80.0);
    assert_eq!(g.find_object(far).unwrap().health(), 100.0);
    assert_eq!(g.find_object(ally).unwrap().health(), 100.0);

    // Aiming at empty ground off to the side hits the ground, not an object.
    let side = Coord3D::new(200.0, 300.0, 0.0);
    assert!(matches!(g.hitscan_target(a, &side, 150.0), AttackTarget::Ground(_)));
}

#[test]
fn death_auto_releases() {
    let (mut g, id) = world();
    g.submit(0, Command::Possess { id });
    g.update();
    assert_eq!(g.possessed_unit(0), Some(id));
    g.damage_object(id, DamageInfo { source: ObjectId::INVALID, amount: 1000.0, kind: DamageType::Unresistable });
    g.run_frames(2);
    assert_eq!(g.possessed_unit(0), None);
    assert!(g.find_object(id).is_none());
    // Input afterwards is ignored.
    g.submit(0, input(1.0, 0.0, Coord3D::ZERO, true));
    g.update();
}

#[test]
fn release_hands_back_to_idle_ai() {
    let (mut g, id) = world();
    g.submit(0, Command::Possess { id });
    for _ in 0..30 {
        g.submit(0, input(1.0, 0.0, Coord3D::ZERO, false));
        g.update();
    }
    g.submit(0, Command::Release);
    g.run_frames(40);
    let o = g.find_object(id).unwrap();
    assert!(!o.ai.suspended && !o.ai.is_moving() && o.combat.attack.is_none());
    assert_eq!(o.ai.speed, 0.0, "AI braked the unit");
}

fn scripted_run(seed: u32) -> Vec<u32> {
    let mut g = GameLogic::new(flat(), seed);
    let a = g.create_object(&tank("A", Some(cannon()), true), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.create_object(&tank("B", None, false), Coord3D::new(320.0, 230.0, 0.0), 0.0, 1);
    g.create_object(&tank("C", Some(cannon()), true), Coord3D::new(500.0, 500.0, 0.0), 0.0, 1);
    let mut crcs = Vec::new();
    g.submit(0, Command::Possess { id: a });
    for f in 0..300u32 {
        let (mx, my) = if f < 60 {
            (1.0, 0.3)
        } else if f < 120 {
            (0.0, 1.0)
        } else {
            (0.0, 0.0)
        };
        let aim = Coord3D::new(320.0, 230.0, 0.0);
        g.submit(0, input(mx, my, aim, f > 40));
        g.update();
        crcs.push(g.crc());
    }
    crcs
}

#[test]
fn two_instances_agree_every_frame() {
    assert_eq!(scripted_run(7), scripted_run(7));
    // And possession actually changes the state.
    let mut plain = GameLogic::new(flat(), 7);
    plain.create_object(&tank("A", Some(cannon()), true), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    plain.run_frames(2);
    assert_ne!(scripted_run(7)[1], plain.crc());
}

// ---- air possession ---------------------------------------------------------------------------

fn hover(name: &str) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.geometry.height = 10.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some({
        let mut l = LocomotorParams::from_ini(LocomotorKind::Hover, 30.0, 60.0, 120.0, 120.0);
        l.behavior_z = ZBehavior::SurfaceRelativeHeight;
        l.stick_to_ground = false;
        l.preferred_height = 30.0;
        l
    });
    Arc::new(t)
}

fn fixed_wing(name: &str) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.geometry.height = 10.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some({
        let mut l = LocomotorParams::from_ini(LocomotorKind::Wings, 60.0, 30.0, 60.0, 90.0);
        l.min_speed = 10.0 / 30.0;
        l.behavior_z = ZBehavior::AbsoluteHeight;
        l.stick_to_ground = false;
        l.preferred_height = 200.0;
        l
    });
    Arc::new(t)
}

fn input_buttons(mx: f32, my: f32, buttons: u8) -> Command {
    Command::PossessInput(PossessInput { move_x: (mx * 32767.0) as i16, move_y: (my * 32767.0) as i16, aim: Coord3D::ZERO, buttons })
}

#[test]
fn hover_climbs_descends_and_drives_horizontally() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&hover("H"), Coord3D::new(200.0, 200.0, 30.0), 0.0, 0);
    g.submit(0, Command::Possess { id });
    for _ in 0..30 {
        g.submit(0, input_buttons(0.0, 0.0, 0));
        g.update();
    }
    let z0 = g.find_object(id).unwrap().position.z;
    for _ in 0..60 {
        g.submit(0, input_buttons(0.0, 0.0, buttons::ASCEND));
        g.update();
    }
    let z1 = g.find_object(id).unwrap().position.z;
    assert!(z1 > z0 + 20.0, "climbs: {z0} -> {z1}");
    // Envelope: surface + 2 .. surface + 2 * cruise height (cruise floors at 60).
    for _ in 0..600 {
        g.submit(0, input_buttons(0.0, 0.0, buttons::ASCEND));
        g.update();
    }
    let zmax = g.find_object(id).unwrap().position.z;
    assert!(zmax <= 121.0, "clamped below the ceiling: {zmax}");
    for _ in 0..600 {
        g.submit(0, input_buttons(0.0, 0.0, buttons::DESCEND));
        g.update();
    }
    let zmin = g.find_object(id).unwrap().position.z;
    assert!((1.0..=3.0).contains(&zmin), "descends to the floor: {zmin}");
    // Horizontal drive works while airborne.
    let x0 = g.find_object(id).unwrap().position.x;
    for _ in 0..30 {
        g.submit(0, input_buttons(1.0, 0.0, 0));
        g.update();
    }
    assert!(g.find_object(id).unwrap().position.x > x0 + 5.0);
}

#[test]
fn wings_throttle_turn_and_never_stop() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&fixed_wing("W"), Coord3D::new(200.0, 200.0, 200.0), 0.0, 0);
    let max = g.find_object(id).unwrap().template.locomotor.unwrap().max_speed;
    let min = g.find_object(id).unwrap().template.locomotor.unwrap().min_speed;
    g.submit(0, Command::Possess { id });
    for _ in 0..90 {
        g.submit(0, input_buttons(0.0, 1.0, 0));
        g.update();
    }
    let o = g.find_object(id).unwrap();
    assert!((o.ai.speed - max).abs() < 1e-3, "reaches top speed: {}", o.ai.speed);
    assert!(o.position.x > 250.0, "flies forward: {}", o.position.x);
    // D turns right (angle increases) while still flying.
    let a0 = g.find_object(id).unwrap().angle;
    g.submit(0, input_buttons(1.0, 1.0, 0));
    g.update();
    assert!(g.find_object(id).unwrap().angle > a0);
    // No input: still flying at least at min speed (cannot stop).
    for _ in 0..90 {
        g.submit(0, input_buttons(0.0, 0.0, 0));
        g.update();
    }
    assert!(g.find_object(id).unwrap().ai.speed >= min - 1e-3);
    // S throttles down but never below min speed.
    for _ in 0..600 {
        g.submit(0, input_buttons(0.0, -1.0, 0));
        g.update();
    }
    assert!((g.find_object(id).unwrap().ai.speed - min).abs() < 1e-3);
}
