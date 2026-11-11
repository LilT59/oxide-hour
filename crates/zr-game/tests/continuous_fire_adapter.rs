//! The continuous-fire and reload numbers of a weapon reach the sim weapon (`adapter::convert_weapon`), on synthetic data.

use zr_game::adapter::convert_weapon;
use zr_templates::WeaponTemplate;

#[test]
fn the_continuous_fire_and_reload_numbers_reach_the_sim_weapon() {
    let mut w = WeaponTemplate::new("Gatling");
    w.continuous_fire_one_shots_needed = 3;
    w.continuous_fire_two_shots_needed = 6;
    w.continuous_fire_coast_frames = 300;
    w.auto_reload_when_idle_frames = 900;
    w.fire_sound_loop_time = 120;
    let s = convert_weapon(&w);
    assert_eq!(
        (
            s.continuous_fire_one_shots_needed,
            s.continuous_fire_two_shots_needed,
            s.continuous_fire_coast_frames,
            s.auto_reload_when_idle_frames,
            s.fire_sound_loop_time,
        ),
        (3, 6, 300, 900, 120)
    );
}

/// `DamageStatusType = IMMOBILE` reaches the sim weapon as the status bit `1 << 42`, which is the sim's `status::IMMOBILE`
/// (the zr-templates `ObjectStatus` index is the C++ `ObjectStatusTypes` order).
#[test]
fn the_damage_status_type_reaches_the_sim_weapon_as_a_status_mask() {
    let mut w = WeaponTemplate::new("Stun");
    w.damage_status_type = zr_templates::ObjectStatus::Immobile;
    let s = convert_weapon(&w);
    assert_eq!(s.damage_status_mask, zr_sim::object::status::IMMOBILE);
    let none = convert_weapon(&WeaponTemplate::new("Gun"));
    assert_eq!(none.damage_status_mask, 0);
}
