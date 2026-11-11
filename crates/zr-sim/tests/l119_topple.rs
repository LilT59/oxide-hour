//! R10 finding 12: `ToppleUpdate` with `KillWhenStartToppling` does `setWakeFrame(FOREVER); kill(); return`
//! (`ToppleUpdate.cpp:137-156`), where `Object::kill` is a normal death of the object's own health
//! (`Object.cpp:2010-2024`), not a `DEATH_TOPPLED` death. Synthetic data only.

use std::sync::Arc;

use zr_sim::damage::DeathType;
use zr_sim::die::{DieMux, DieModuleTemplate, DieTemplate, SlowDeathTemplate};
use zr_sim::modules2::ToppleTemplate;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(200, 200, 0)
}

/// A `SlowDeath` die module that only applies to a `DEATH_NORMAL` death, so a `DEATH_TOPPLED` kill finds no
/// die module at all.
fn slow_death_on_normal(frames: u32) -> DieModuleTemplate {
    DieModuleTemplate {
        mux: DieMux { death_types: 1 << (DeathType::Normal as u32), veterancy_levels: u32::MAX, exempt_status: 0, required_status: 0 },
        kind: DieTemplate::SlowDeath(Arc::new(SlowDeathTemplate {
            probability_modifier: 1,
            modifier_bonus_per_overkill_percent: 0.0,
            sink_rate: 0.0,
            sink_delay: 0,
            sink_delay_variance: 0,
            destruction_delay: frames,
            destruction_delay_variance: 0,
            destruction_altitude: -10.0,
            fx: [Vec::new(), Vec::new(), Vec::new()],
            ocl: [Vec::new(), Vec::new(), Vec::new()],
            weapons: [Vec::new(), Vec::new(), Vec::new()],
            fling_force: 0.0,
            fling_force_variance: 0.0,
            fling_pitch: 0.0,
            fling_pitch_variance: 0.0,
            special: None,
        })),
    }
}

/// `if (d->m_killWhenStartToppled) { setWakeFrame(FOREVER); getObject()->kill(); return; }`
/// (`ToppleUpdate.cpp:150-155`): the object dies `DEATH_NORMAL` with its own health, the slow death that takes
/// only a normal death runs, and the topple module is never entered again (`m_toppleState` stays upright).
/// Revert check: kill with `DEATH_TOPPLED` and `HUGE_DAMAGE_AMOUNT` and mark the port's `toppled` flag (the
/// pre-R10 code); no die module applies, the object lingers and `toppled` is set.
#[test]
fn a_kill_when_start_toppling_uses_the_normal_death() {
    let mut g = GameLogic::new(flat(), 1);
    let mut t = ObjectTemplate::new("Tree");
    t.kind_of = kind_of::VEHICLE;
    t.geometry.major_radius = 5.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t.topple = Some(ToppleTemplate { kill_when_start_toppling: true, kill_when_finished_toppling: false, topple_left_or_right_only: false });
    t.die = vec![slow_death_on_normal(5)];
    let id = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);

    g.topple(id, false);
    let o = g.find_object(id).expect("the slow death keeps the body for a while");
    assert!(!o.behavior.toppled, "C++ leaves m_toppleState upright (ToppleUpdate.cpp:150-155)");
    assert_eq!(o.combat.death, Some(DeathType::Normal), "Object::kill is DEATH_NORMAL (Object.cpp:2010-2024)");
    assert!(o.is_dying(), "the object is killed at once");

    g.run_frames(20);
    assert!(g.find_object(id).is_none(), "the normal-death slow death destroys the body");
}
