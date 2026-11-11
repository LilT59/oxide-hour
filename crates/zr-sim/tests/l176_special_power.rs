//! Lane 176 (R20-4): `SpecialAbilityUpdate::endPreparation` clears `OBJECT_STATUS_IS_USING_ABILITY`
//! (`SpecialAbilityUpdate.cpp:1961-1966`) right after `triggerAbilityEffect` (`:365-383`), before the pack
//! starts. With a pack time the ability stays active for the pack's length, so the bit is the only way to
//! see the preparation end.

use std::collections::BTreeMap;
use std::sync::Arc;
use zr_sim::ai_states::AiStateType;
use zr_sim::damage::DamageType;
use zr_sim::object::status;
use zr_sim::special_power::{SimSpecialPowerTemplate, SpecialAbilityTemplate, SpecialPowerTarget};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

const POWER: &str = "Capture";

fn install_capture(g: &mut GameLogic) {
    g.set_special_powers(BTreeMap::from([(
        POWER.to_string(),
        Arc::new(SimSpecialPowerTemplate {
            name: POWER.to_string(),
            kind: "SPECIAL_INFANTRY_CAPTURE_BUILDING".to_string(),
            reload_time: 0,
            required_science: None,
            shared_synced_timer: false,
            public_timer: false,
            radius_cursor_radius: 0.0,
        }),
    )]));
}

fn vehicle(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    let mut w = WeaponTemplate::new("Cannon");
    w.primary_damage = 0.0;
    w.damage_type = DamageType::Explosion;
    w.attack_range = 300.0;
    w.min_delay_between_shots = 1000;
    w.max_delay_between_shots = 1000;
    let mut s = WeaponSetTemplate::default();
    s.weapons[0] = Some(Arc::new(w));
    t.combat = CombatTemplate { weapon_set: Some(s), ..CombatTemplate::default() };
    t
}

/// A capture unit whose preparation is instant and whose pack takes 60 frames: the trigger and
/// `startPacking(true)` happen on the same update, and the ability then stays active for the pack.
fn capture_unit() -> ObjectTemplate {
    let no_timings = SpecialAbilityTemplate {
        power: POWER.to_string(),
        start_ability_range: 200.0,
        ability_abort_range: 10_000_000.0,
        approach_requires_los: true,
        preparation_frames: 0,
        pack_time: 0,
        unpack_time: 0,
        pack_unpack_variation: 0.0,
        skip_packing_with_no_target: false,
        award_xp_for_triggering: 0,
        skill_points_for_triggering: -1,
        effect_duration: 0,
        effect_value: 1,
        special_object: None,
        max_special_objects: 1,
        need_to_face: false,
        flee_range_after_completion: 0.0,
        starts_paused: false,
        power_module: true,
    };
    let update = SpecialAbilityTemplate {
        preparation_frames: 0,
        pack_time: 60,
        power_module: false,
        ..no_timings.clone()
    };
    let mut t = vehicle("Ranger");
    t.special_abilities = vec![Arc::new(no_timings), Arc::new(update)];
    t
}

#[test]
fn the_capture_clears_using_ability_when_its_preparation_ends_and_packs() {
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    // Players 0 and 1 with their default teams: the capture's colleague check compares team instances (F6).
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.scripts_mut().set_player_name("p0", 0);
    g.scripts_mut().set_player_name("p1", 1);
    g.scripts_mut().load_default_team_protos(&[("teamP0", "p0"), ("teamP1", "p1")]);
    g.init_from_sides(&[("teamP0", 0), ("teamP1", 1)]);
    install_capture(&mut g);
    let a = g.create_object(&Arc::new(capture_unit()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(vehicle("Bldg")), Coord3D::new(150.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::DoSpecialPower { source: a, power: POWER.into(), target: SpecialPowerTarget::Object(b) });
    let mut triggered = false;
    for _ in 0..40 {
        g.update();
        if g.find_object(b).unwrap().player == 0 {
            let a_obj = g.find_object(a).unwrap();
            // `endPreparation()` ran after the trigger; the ability itself is still active (packing).
            assert_eq!(
                a_obj.status_mask(g.frame()) & status::USING_ABILITY,
                0,
                "IS_USING_ABILITY is cleared when the preparation ends, not when the pack ends"
            );
            assert!(a_obj.ability.is_some(), "the ability is still active while it packs");
            assert_eq!(a_obj.ai.machine.current, AiStateType::Busy, "`startPacking` runs `aiBusy`");
            triggered = true;
            break;
        }
    }
    assert!(triggered, "the capture triggers within 40 frames");
}
