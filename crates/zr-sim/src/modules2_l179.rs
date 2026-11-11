//! Lane 179 (R21-20): `SlavedUpdate::update` builds the pinned point from the master's **position** and changes only
//! x and y; `m_guardPointOffset.z` takes the ground height, but `pinnedPosition.z` stays the master's z
//! (SlavedUpdate.cpp:255-262). The 3D guard test `getDistanceSquared(me, &pinnedPosition, FROM_CENTER_3D)`
//! (:266-273) then sees the master's height, so an idle slave under an airborne master is sent back to the point.
//! Revert check: `pinned.z = ground` (the port before this lane) leaves the slave idle.

use super::*;
use crate::ai_states::AiStateType;
use crate::math::Coord3D;
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
use crate::terrain::TerrainLogic;
use std::sync::Arc;

fn dummy(name: &str, slaved: Option<SlavedTemplate>) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.vision_range = 40.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 240.0));
    t.slaved = slaved;
    Arc::new(t)
}

#[test]
fn an_idle_slave_is_pulled_back_to_the_point_under_the_masters_height() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    let master = g.create_object(&dummy("Master", None), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let slaved = SlavedTemplate {
        guard_max_range: 30,
        guard_wander_range: 0,
        attack_range: 0,
        repair_rate_per_second: 0.0,
        stay_on_same_layer_as_master: false,
    };
    let slave = g.create_object(&dummy("Slave", Some(slaved)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.slaved_on_enslave(slave, master);
    // The master hovers 20 above the ground; the pinned offset is zero, so the pinned point is (100, 100, 20).
    if let Some(o) = g.objects.get_mut(&master) {
        o.set_position(Coord3D::new(100.0, 100.0, 20.0));
    }
    if let Some(o) = g.objects.get_mut(&slave) {
        o.behavior.slaved_guard_offset = Coord3D::new(0.0, 0.0, 0.0);
        o.ai.machine.current = AiStateType::Idle;
    }
    g.slaved_leash(slave, master);
    let o = g.find_object(slave).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::MoveTo, "the 3D distance to (100, 100, 20) is 20, more than CLOSE_ENOUGH (15)");
    assert_eq!(o.behavior.slaved_guard_offset.z, 0.0, "m_guardPointOffset.z is the ground height at the pinned point (SlavedUpdate.cpp:259)");
}
