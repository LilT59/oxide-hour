//! Lane 125: `TechBuildingBehavior`'s `PulseFX`/`PulseFXRate` (`TechBuildingBehavior.cpp:52-118`).
//!
//! The constructor asks for one evaluation (`:76`). Its `update` sets/clears `MODELCONDITION_CAPTURED` from the
//! controlling player (`:92-104`) and, when the module data has a `PulseFX` and a positive `PulseFXRate` and the
//! building is captured, plays the FX and sleeps `PulseFXRate` frames; otherwise it sleeps forever (`:106-117`).
//! The FX is a client effect record in the sim (`FXList::doFXObj`).

use std::sync::Arc;
use zr_sim::template::{kind_of, SleepyModuleKind, TechBuildingTemplate};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn derrick(pulse: Option<(&str, u32)>) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("TechDerrick");
    t.kind_of = kind_of::STRUCTURE;
    t.max_health = 500.0;
    t.initial_health = 500.0;
    t.tech_building = pulse.map(|(name, rate)| TechBuildingTemplate {
        pulse_fx: Some(name.to_owned()),
        pulse_fx_rate: rate,
    });
    t.update_module_order = vec![SleepyModuleKind::TechBuilding];
    t
}

/// A captured building pulses its FX every `PulseFXRate` frames and sleeps that long between pulses; giving it to the
/// neutral player clears `CAPTURED` and puts the module back to sleep. Revert check: with the old stub the FX is
/// never recorded and the wake stays at forever.
#[test]
fn a_captured_tech_building_pulses_its_fx_at_the_pulse_rate() {
    use zr_sim::module::UPDATE_SLEEP_FOREVER;
    let mut g = GameLogic::new(TerrainLogic::flat(80, 80, 0), 1);
    g.set_neutral_player(Some(9));
    g.add_player(0, 0);
    g.add_player(9, 0);
    let t = Arc::new(derrick(Some(("FX_Pulse", 4))));
    let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let handle = g.find_object(id).unwrap().behavior.tech_handle.expect("tech building slot");
    // First evaluation one frame after creation (`TechBuildingBehavior.cpp:76`).
    g.run_frames(2);
    assert!(g.find_object(id).unwrap().behavior.tech_captured, "player 0 is not the neutral player");
    assert!(g.fx_events().iter().any(|e| e.name == "FX_Pulse"), "the pulse FX is recorded");
    assert_eq!(g.update_wake_frame(handle), 5, "the first evaluation (frame 1) sleeps PulseFXRate = 4 frames");
    // Hand it to the neutral player: `CAPTURED` clears and the module sleeps forever.
    g.set_object_team(id, 9);
    g.run_frames(5);
    assert!(!g.find_object(id).unwrap().behavior.tech_captured);
    assert_eq!(g.update_wake_frame(handle), UPDATE_SLEEP_FOREVER, "no pulse without an owner");
}

/// The module data's `PulseFXRate` must be positive to pulse (`TechBuildingBehavior.cpp:108-112`).
#[test]
fn a_zero_pulse_rate_sleeps_forever() {
    use zr_sim::module::UPDATE_SLEEP_FOREVER;
    let mut g = GameLogic::new(TerrainLogic::flat(80, 80, 0), 1);
    g.add_player(0, 0);
    let t = Arc::new(derrick(Some(("FX_Pulse", 0))));
    let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.run_frames(2);
    assert!(!g.fx_events().iter().any(|e| e.name == "FX_Pulse"), "no pulse at rate 0");
    let handle = g.find_object(id).unwrap().behavior.tech_handle.expect("tech building slot");
    g.run_frames(1);
    assert_eq!(g.update_wake_frame(handle), UPDATE_SLEEP_FOREVER);
}
