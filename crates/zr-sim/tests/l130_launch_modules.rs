//! Lane 130: the launch-bone tables merge over every draw module (`Drawable::getProjectileLaunchOffset`,
//! Drawable.cpp:662-671) and the `NIGHT`/`SNOW` model-condition bits reach the fire-time query
//! (`Object::friend_bindToDrawable`, Object.cpp:3038-3048). Synthetic data only: the templates below stand in for the
//! GLA Technical (launch bones in a later draw module) and a night/snow-only model state.

use std::sync::Arc;

use zr_sim::projectile::{MissileParams, ProjectileBehavior, ProjectileTemplate};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, LaunchBarrel, LaunchBones, LaunchModule, LaunchState, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(300, 300, 0)
}

fn gun() -> WeaponTemplate {
    let mut w = WeaponTemplate::new("W");
    w.primary_damage = 10.0;
    w.primary_damage_radius = 2.0;
    w.attack_range = 500.0;
    w.has_projectile = true;
    w.weapon_speed = 5.0;
    w.projectile = Some(Arc::new(ProjectileTemplate {
        behavior: ProjectileBehavior::Missile(MissileParams { initial_velocity: 5.0, ..MissileParams::default() }),
        bounding_radius: 3.0,
        small_missile: false,
        neutron_warhead: None,
        neutron_blast: None,
        garrison_hit_kill: None,
    }));
    w
}

fn barrel(z: f32) -> LaunchBarrel {
    LaunchBarrel { pos: [0.0, 0.0, z], rot: [0.0, 0.0, 0.0, 1.0] }
}

/// A state whose one weapon slot is `slot`, at `z`, selected by `conditions` (an all-zero entry is the default state).
fn state(conditions: Vec<u128>, slot: usize, z: f32) -> LaunchState {
    let mut slots: [Vec<LaunchBarrel>; 3] = Default::default();
    slots[slot] = vec![barrel(z)];
    LaunchState { conditions, slots, turret: None }
}

/// The default state with no barrels at all (`validateWeaponBarrelInfo` runs but finds no `WeaponLaunchBone`).
fn empty_state() -> LaunchState {
    LaunchState { conditions: vec![0], slots: Default::default(), turret: None }
}

/// A tank with the given launch-bone tables: `states` is the first launch-capable module, `extra` the later ones.
fn tank(launch: LaunchBones) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Tank");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 200.0;
    t.initial_health = 200.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(gun()));
    t.combat = CombatTemplate { weapon_set: Some(set), launch_bones: launch, ..CombatTemplate::default() };
    t
}

/// Attacker (player 0) with `t` at (100, 300), a victim (player 1) 60 units east. Returns the launch position of the
/// first shot, i.e. the projectile's `prev_pos` on its creation frame (`fireWeaponTemplate` starts it at `launch`).
fn first_launch_position(t: ObjectTemplate, env: Option<(bool, bool)>) -> Coord3D {
    let mut g = GameLogic::new(flat(), 1);
    if let Some((night, snow)) = env {
        g.set_launch_environment(night, snow);
    }
    let a = g.create_object(&Arc::new(t), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(ObjectTemplate::new("Target")), Coord3D::new(160.0, 300.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    for _ in 0..180 {
        g.run_frames(1);
        if let Some(p) = g.projectiles().first() {
            return p.prev_pos;
        }
    }
    panic!("the tank never fired (attacker {a:?}, target {b:?})");
}

/// `Drawable::getProjectileLaunchOffset` walks every draw module: the truck-body module has no barrel and the turret
/// module's state does, so the shot starts at the turret bone instead of the object origin. This is the GLA Technical's
/// crate-upgrade case. Reverting `LaunchBones::select_slot` to the first module's table (`select`) fails it.
#[test]
fn a_later_draw_module_answers_the_launch_slot() {
    let lb = LaunchBones {
        states: vec![empty_state()],
        extra_modules: vec![LaunchModule { ignore: 0, states: vec![state(vec![0], 0, 9.0)] }],
        ..LaunchBones::default()
    };
    let p = first_launch_position(tank(lb), None);
    assert!((p.z - 9.0).abs() < 1e-3, "the second module's bone launches the shot (z {})", p.z);
}

/// The module walk is per weapon slot: a module whose state has no barrel *for the fired slot* is skipped, so the primary
/// slot can come from a later module while the secondary slot still comes from the first. Reverting the per-slot
/// emptiness check fails the primary assert.
#[test]
fn the_module_walk_is_per_weapon_slot() {
    let lb = LaunchBones {
        states: vec![state(vec![0], 1, 1.0)],
        extra_modules: vec![LaunchModule { ignore: 0, states: vec![state(vec![0], 0, 5.0)] }],
        ..LaunchBones::default()
    };
    assert_eq!(lb.select_slot(1, 0).barrels[0].pos[2], 1.0, "the first module answers the secondary slot");
    let p = first_launch_position(tank(lb), None);
    assert!((p.z - 5.0).abs() < 1e-3, "the primary slot comes from the second module (z {})", p.z);
}

/// `Object::friend_bindToDrawable` (Object.cpp:3038-3048) puts the global time-of-day / weather into every drawable's
/// model conditions, and the launch state is chosen from them. A NIGHT state fires from 12 up and a SNOW state from 20;
/// with neither bit the default states answer (no barrel) and the shot starts at the object's origin. Reverting
/// `launch_conditions`'s environment bits fails the two non-zero cases.
#[test]
fn night_and_snow_select_the_launch_state() {
    let bit = |n: &str| 1u128 << zr_sim::condition::bit_for_name(n).unwrap();
    let template = || LaunchBones {
        states: vec![empty_state(), state(vec![bit("NIGHT")], 0, 12.0)],
        extra_modules: vec![LaunchModule { ignore: 0, states: vec![empty_state(), state(vec![bit("SNOW")], 0, 20.0)] }],
        ..LaunchBones::default()
    };
    assert_eq!(first_launch_position(tank(template()), None).z, 0.0, "no condition: no module has a barrel");
    let night = first_launch_position(tank(template()), Some((true, false)));
    assert!((night.z - 12.0).abs() < 1e-3, "NIGHT picks the night state (z {})", night.z);
    let snow = first_launch_position(tank(template()), Some((false, true)));
    assert!((snow.z - 20.0).abs() < 1e-3, "SNOW picks the snow state (z {})", snow.z);
    let both = first_launch_position(tank(template()), Some((true, true)));
    assert!((both.z - 12.0).abs() < 1e-3, "the earlier NIGHT module still answers first (z {})", both.z);
}

/// The environment is a game-wide value: `GameLogic::launch_environment` keeps what the map loader set (afternoon /
/// normal by default, `GlobalData.cpp:725-730`).
#[test]
fn the_launch_environment_is_game_wide() {
    let mut g = GameLogic::new(flat(), 1);
    assert!(!g.launch_environment().night && !g.launch_environment().snow, "the engine default is afternoon/normal");
    g.set_launch_environment(true, true);
    assert!(g.launch_environment().night && g.launch_environment().snow);
}
