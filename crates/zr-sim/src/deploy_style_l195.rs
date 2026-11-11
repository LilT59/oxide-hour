//! `DeployStyleAIUpdate::update` (lane 195, wave 23): it calls `AIUpdateInterface::update()` and then returns
//! `UPDATE_SLEEP_NONE` ("We can't sleep the deploy AI because any new commands need to be caught", DeployStyleAIUpdate.cpp:
//! 274-277), so the parent's sub-sleep is discarded and the module is awake every frame. Only this sleep return is
//! ported; the module's deploy/undeploy state machine (DeployStyleAIUpdate.cpp:95-277) is not. AN3 seed 3's sentry drone 253 is
//! idle and parked: C++ re-keys its AI module to the next frame each time (f2486 key 9948), and the port slept it to 2489.

use std::sync::Arc;

use crate::logic::GameLogic;
use crate::math::Coord3D;
use crate::module::UPDATE_SLEEP_NONE;
use crate::object::ObjectId;
use crate::template::ObjectTemplate;
use crate::terrain::TerrainLogic;

fn logic() -> GameLogic {
    GameLogic::new(TerrainLogic::flat(100, 100, 0), 1)
}

/// An immobile, parked AI unit. `IMMOBILE` makes `doLocomotor`'s sleep FOREVER, so the AI's own return is the state
/// machine's sleep, which is what a deploy-style unit must not keep.
fn parked_ai_unit(deploy_style: bool) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("SentryDrone");
    t.kind_of = crate::template::kind_of::VEHICLE | crate::template::kind_of::SELECTABLE | crate::template::kind_of::IMMOBILE;
    t.geometry.major_radius = 5.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.ai_update = Some(0);
    t.deploy_style_ai = deploy_style;
    t
}

/// The heap key's frame of the AI module of `id`.
fn ai_key_frame(g: &GameLogic, id: ObjectId) -> u32 {
    let h = g.objects[&id].ai_handle.expect("an AI module");
    g.schedule.slot(h.0).next_call_and_phase >> 2
}

/// After a few frames the idle machine sleeps: a plain immobile unit's AI module is keyed to its state's sleep, but a
/// deploy-style unit's AI module is keyed to the next frame. Revert check: without the `deploy_style_ai` return in
/// `ai_update_sleep` the drone's key equals the plain unit's (the state sleep) and the first assert fails.
#[test]
fn a_deploy_style_ai_module_is_awake_every_frame() {
    let mut g = logic();
    g.add_player(0, 0);
    let plain = g.create_object(&Arc::new(parked_ai_unit(false)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let drone = g.create_object(&Arc::new(parked_ai_unit(true)), Coord3D::new(300.0, 300.0, 0.0), 0.0, 0);
    for _ in 0..5 {
        g.update();
    }
    let now = g.frame();
    g.update();
    assert_eq!(ai_key_frame(&g, drone), now + UPDATE_SLEEP_NONE, "a deploy-style AI is awake the next frame");
    assert!(ai_key_frame(&g, plain) > now + UPDATE_SLEEP_NONE, "a plain immobile AI sleeps for its state");
}
