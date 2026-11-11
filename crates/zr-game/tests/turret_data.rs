//! The shipped `TurretAIData` fields the port's `TurretAI` state machine reads, checked against the
//! C++ RNG trace of AN3 s1 (`ZR_RNG_TRACE`, `TurretAI.cpp:1289, 1370-1371`). Skips when no install.

use std::sync::Arc;
use zr_assets::{Installs, Vfs};
use zr_game::GameData;
use zr_sim::template::TemplateSource;

fn vfs() -> Option<Vfs> {
    let zh = Installs::detect().zero_hour?;
    Vfs::zero_hour(&zh).ok()
}

#[test]
fn shipped_turret_fields_match_the_cpp_trace() {
    let Some(vfs) = vfs() else { return };
    let data = GameData::load(&vfs);
    let c = |name: &str| data.store.find_template(name).unwrap_or_else(|| panic!("{name}")).combat.clone();

    // AN3 s1 C++ RNG draws (`cpp_rng_l63e.txt`): the first turret draw is `n=1784 f=2516
    // lo=9999999 hi=9999999` at the `AmericaVehicleHumvee` creation; the `AmericaFireBase`
    // creation draws `n=1901 f=2859 lo=17 hi=29` and its scan `n=1920 f=2882 k=R lo=0
    // hi=40c90fdb` (2*pi) + `n=1921 k=I lo=0 hi=1`.
    let humvee = c("AmericaVehicleHumvee");
    assert_eq!(humvee.turret_min_idle_scan_interval, 9_999_999, "Humvee: the default interval");
    assert_eq!(humvee.turret_max_idle_scan_interval, 9_999_999);
    assert_eq!(humvee.turret_min_idle_scan_angle, 0.0);
    assert_eq!(humvee.turret_max_idle_scan_angle, 0.0);
    assert!(humvee.turret_turn_rate.is_some(), "Humvee has a turret");

    let firebase = c("AmericaFireBase");
    assert_eq!(firebase.turret_min_idle_scan_interval, 17, "FireBase MinIdleScanInterval");
    assert_eq!(firebase.turret_max_idle_scan_interval, 29, "FireBase MaxIdleScanInterval");
    assert!((firebase.turret_max_idle_scan_angle - 2.0 * std::f32::consts::PI).abs() < 1e-5, "FireBase MaxIdleScanAngle = 360 degrees ({})", firebase.turret_max_idle_scan_angle);
    assert_eq!(firebase.turret_min_idle_scan_angle, 0.0);
    assert!(firebase.turret_recenter_time > 0, "RecenterTime parsed");

    let scorpion = c("GLATankScorpion");
    assert!(scorpion.turret_turn_rate.is_some());
    assert_eq!(scorpion.turret_min_idle_scan_interval, 9_999_999, "Scorpion: the default interval");
    assert_eq!(scorpion.turret_controlled_slots & 1, 1, "the primary weapon is on the turret");

    // A Crusader's turret turns and pitches; its pitch fields feed the AIM state's pitch block.
    let crusader = c("AmericaTankCrusader");
    assert!(crusader.turret_turn_rate.unwrap() > 0.0);
    assert!(crusader.turret_pitch_rate.is_some());
    assert_eq!(crusader.turret_controlled_slots & 1, 1, "the primary weapon is on the turret");
}

/// A controlled scene: one `AmericaFireBase` (AN3 s1's obj 261, the first turreted object whose
/// idle scan runs: C++ creation f=2859, scan f=2882) idle on the real map. Its `IDLE -> IDLESCAN ->
/// HOLD -> RECENTER -> IDLE` cycle must use the template's 17..29 frame interval, the 0..2*pi scan
/// angle and the `RecenterTime` hold, in the C++ draw order.
#[test]
fn a_lone_firebase_runs_the_idle_scan_cycle() {
    let Some(vfs) = vfs() else { return };
    let data = Arc::new(GameData::load(&vfs));
    let setups = [zr_game::PlayerSetup::new("FactionAmerica", 0), zr_game::PlayerSetup::new("FactionChina", 1)];
    let mut g = zr_game::Game::new_skirmish_with(data.clone(), &vfs, "Alpine Assault", &setups, 1).unwrap();
    // Owned by the neutral civilian player, so no enemy can pull it into AIM.
    let id = g.spawn_object("AmericaFireBase", zr_sim::Coord3D::new(400.0, 3000.0, 0.0), 0.0, 1).expect("FireBase");
    let turret = |g: &zr_game::Game| g.logic().find_object(id).unwrap().combat.turret.clone().unwrap();
    assert_eq!(turret(&g).state, zr_sim::turret_ai::TurretStateType::Idle);
    let created = g.logic().frame();
    let scan_at = turret(&g).next_idle_scan;
    assert!((17..=29).contains(&(scan_at - created)), "the creation draw is the 17..29 interval ({})", scan_at - created);
    // Run to the scan (a state change happens within the interval).
    let mut scan_frame = None;
    for _ in 0..40 {
        g.step();
        if turret(&g).state == zr_sim::turret_ai::TurretStateType::IdleScan {
            scan_frame = Some(g.logic().frame());
            break;
        }
    }
    let scan_frame = scan_frame.expect("the idle scan starts");
    assert!((scan_at..=scan_at + 1).contains(&scan_frame), "the scan starts at m_nextIdleScan ({scan_frame} vs {scan_at})");
    assert!((0.0..=2.0 * std::f32::consts::PI).contains(&turret(&g).desired_angle.abs()), "the scan angle is in 0..2*pi");
    // It turns to the scan angle, holds for RecenterTime, recenters and returns to idle.
    let mut saw_hold = false;
    let mut saw_recenter = false;
    for _ in 0..400 {
        g.step();
        match turret(&g).state {
            zr_sim::turret_ai::TurretStateType::Hold => saw_hold = true,
            zr_sim::turret_ai::TurretStateType::Recenter => saw_recenter = true,
            zr_sim::turret_ai::TurretStateType::Idle if saw_hold && saw_recenter => return,
            _ => {}
        }
    }
    panic!("the cycle did not complete (hold {saw_hold}, recenter {saw_recenter})");
}

