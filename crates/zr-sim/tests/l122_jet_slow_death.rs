//! R10 finding 13: `JetSlowDeathBehavior::beginSlowDeath` runs the base
//! `SlowDeathBehavior::beginSlowDeath` first, which ends in `doPhaseStuff(SDPHASE_INITIAL)`
//! (`SlowDeathBehavior.cpp:329-330`), and only then the jet's own initial FX/OCL
//! (`JetSlowDeathBehavior.cpp:180-193`). Synthetic data, no game install.

use std::sync::Arc;

use zr_sim::body::{DamageInfo, DamageType};
use zr_sim::damage::DeathType;
use zr_sim::die::{DieMux, DieModuleTemplate, DieTemplate, JetSlowDeathTemplate, SlowDeathTemplate};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn normal_only() -> DieMux {
    DieMux { death_types: 1 << (DeathType::Normal as u32), veterancy_levels: u32::MAX, exempt_status: 0, required_status: 0 }
}

fn jet_slow_death() -> DieTemplate {
    let slow = SlowDeathTemplate {
        probability_modifier: 1,
        modifier_bonus_per_overkill_percent: 0.0,
        sink_rate: 0.0,
        sink_delay: 0,
        sink_delay_variance: 0,
        destruction_delay: 1000,
        destruction_delay_variance: 0,
        destruction_altitude: -10.0,
        // Two INITIAL FX: `doPhaseStuff` draws an index, so the base phase also draws from the RNG.
        fx: [vec!["BaseInitialLow".into(), "BaseInitialHigh".into()], Vec::new(), Vec::new()],
        ocl: [Vec::new(), Vec::new(), Vec::new()],
        weapons: [Vec::new(), Vec::new(), Vec::new()],
        fling_force: 0.0,
        fling_force_variance: 0.0,
        fling_pitch: 0.0,
        fling_pitch_variance: 0.0,
        special: None,
    };
    DieTemplate::JetSlowDeath(Arc::new(JetSlowDeathTemplate {
        slow: Arc::new(slow),
        fx_on_ground_death: Vec::new(),
        ocl_on_ground_death: Vec::new(),
        fx_initial_death: vec!["JetInitial".into()],
        ocl_initial_death: Vec::new(),
        delay_secondary_from_initial_death: 1000,
        fx_secondary: Vec::new(),
        ocl_secondary: Vec::new(),
        fx_hit_ground: Vec::new(),
        ocl_hit_ground: Vec::new(),
        delay_final_blow_up_from_hit_ground: 1000,
        fx_final_blow_up: Vec::new(),
        ocl_final_blow_up: Vec::new(),
        roll_rate: 0.0,
        roll_rate_delta: 1.0,
        pitch_rate: 0.0,
        fall_how_fast: 0.0,
    }))
}

#[test]
fn a_jet_runs_the_base_initial_phase_before_its_own_initial_fx() {
    let mut t = ObjectTemplate::new("TestJet");
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.die = vec![DieModuleTemplate { mux: normal_only(), kind: jet_slow_death() }];
    let t = Arc::new(t);

    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
    // In the air (`JetSlowDeathBehavior::onDie` takes the ground branch only near the ground).
    let id = g.create_object(&t, Coord3D::new(10.0, 10.0, 200.0), 0.0, 1);
    g.damage_object_with_death(id, DamageInfo { source: zr_sim::ObjectId::INVALID, amount: 9999.0, kind: DamageType::Explosion }, DeathType::Normal);

    let fx: Vec<&str> = g.fx_events().iter().map(|e| e.name.as_str()).collect();
    // The base class's INITIAL FX (one of the two, picked by a draw) comes first.
    assert!(
        matches!(fx.first(), Some(&"BaseInitialLow") | Some(&"BaseInitialHigh")),
        "the base SlowDeathBehavior INITIAL phase must run first, got {fx:?}"
    );
    assert_eq!(fx.get(1), Some(&"JetInitial"), "the jet's own FXInitialDeath follows the base's phase, got {fx:?}");
    assert_eq!(fx.len(), 2, "one base INITIAL FX and the jet's FXInitialDeath, got {fx:?}");
}
