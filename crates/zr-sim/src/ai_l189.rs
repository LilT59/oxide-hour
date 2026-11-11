//! Lane 189 (wave 21, the R25 review). Crate-internal tests; each names its finding and the revert check it was run with.
//! - R25-1: an area guard's inner and aggressor entries read the guard position, which C++ leaves at (0,0,0) for an area
//!   (AIStates.cpp:6744-6747 only calls `setAreaToGuard`; AIGuard.cpp:168). Only the scan, the outer circle and the return
//!   move read the polygon's centre (AIGuard.cpp:282-283, :549-551, :656-657).
//! - R25-2: a privateGuardArea clears the machine for a flying jet (AIUpdate.cpp:4169; the jet's aiDoCommand reaches the
//!   base function, JetAIUpdate.cpp:2666-2677), and a grounded jet's default branch clears before taking off
//!   (JetAIUpdate.cpp:2653-2655).

use crate::ai::AiTunables;
use crate::ai_states::AiStateType;
use crate::jet_ai::jet_flag;
use crate::math::Coord3D;
use crate::modules2::JetAiTemplate;
use crate::orders::GuardMode;
use crate::template::{kind_of, LocomotorKind, LocomotorParams, LocomotorSet, ObjectTemplate};
use crate::terrain::TerrainLogic;
use crate::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use crate::GameLogic;
use std::sync::Arc;

/// An armed vehicle with a vision range of 50: its human inner circle is `GuardInnerModifierHuman` (1.8) times that, 90.
fn armed(name: &str) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.vision_range = 50.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 120.0, 120.0, 360.0));
    let mut w = WeaponTemplate::new("Cannon");
    w.primary_damage = 20.0;
    w.damage_type = crate::body::DamageType::ArmorPiercing;
    w.attack_range = 40.0;
    w.min_delay_between_shots = 10;
    w.max_delay_between_shots = 10;
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(w));
    t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    Arc::new(t)
}

/// A weaponless, very tough vehicle: an enemy that never fights back.
fn dummy() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Dummy");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.vision_range = 40.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 240.0));
    Arc::new(t)
}

fn world() -> GameLogic {
    let mut g = GameLogic::new(TerrainLogic::flat(800, 800, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    g.set_ai_tunables(AiTunables { guard_enemy_scan_rate: 30, guard_enemy_return_scan_rate: 30, ..AiTunables::default() });
    g
}

/// The polygon of the tests: the square (50,50)-(550,550), centre (300,300).
fn zone() -> Arc<zr_map::PolygonTrigger> {
    Arc::new(zr_map::PolygonTrigger {
        name: "zone".into(),
        layer: String::new(),
        id: 1,
        is_water: false,
        is_river: false,
        river_start: 0,
        points: vec![[50, 50, 0], [550, 50, 0], [550, 550, 0], [50, 550, 0]],
    })
}

/// R25-1 (AIStates.cpp:6744-6747, AIGuard.cpp:168, :419, :426): an area guard's inner exit circle is centred on the
/// guard position, which an area never sets, so it stays at the origin. The rebel is 60 from the polygon's centre
/// (inside the 90 inner circle) and 468 from the origin (outside it). C++ therefore exits the inner attack at once
/// (`shouldExit`, AIStates.cpp:5526-5527) and the guard's first state after the return is the outer circle. The port
/// centred the inner circle on the polygon and attacked inside `GuardInner`. Revert check: copy the area centre into
/// `position_to_guard` again (the `goal_position` copy in `ai_guard_enter`) and the first state is `GuardInner`.
#[test]
fn an_area_guards_inner_circle_is_centred_on_the_origin_as_the_original_leaves_it() {
    let mut g = world();
    let guard = g.create_object(&armed("Gun"), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let _enemy = g.create_object(&dummy(), Coord3D::new(360.0, 300.0, 0.0), 0.0, 1);
    g.command_guard_area(0, &[guard], zone(), GuardMode::Normal);
    let mut first_after_return = None;
    for _ in 0..200 {
        g.note_trigger_area_change();
        g.update();
        let sub = g.find_object(guard).unwrap().ai.machine.guard_sub;
        if sub != AiStateType::GuardReturn {
            first_after_return = Some(sub);
            break;
        }
    }
    assert_eq!(first_after_return, Some(AiStateType::GuardOuter), "the inner exit circle is at (0,0,0), so the rebel is outside it");
    assert_eq!(g.find_object(guard).unwrap().ai.machine.guard.position_to_guard, Coord3D::ZERO, "an area keeps m_positionToGuard at zero");
}

/// A jet with a flying airframe (`ALLOW_AIR_LOCO`) in the middle of a move: `JetAIUpdate::aiDoCommand` falls through to
/// `AIUpdateInterface::aiDoCommand`, so `privateGuardArea` clears the machine (AIUpdate.cpp:4169, JetAIUpdate.cpp:2666-2677).
/// Its stale goal path must go. Revert check: keep the `!is_jet` exemption in `ai_set_guard_area` and the path survives.
#[test]
fn a_flying_jet_guard_area_clears_the_machine_it_replaces() {
    let mut g = world();
    let jet = g.create_object(&jet_template(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    if let Some(o) = g.objects.get_mut(&jet) {
        o.jet.flags |= jet_flag::ALLOW_AIR_LOCO;
        o.ai.machine.goal_path = vec![Coord3D::new(150.0, 100.0, 0.0)];
    }
    g.command_guard_area(0, &[jet], zone(), GuardMode::Normal);
    let j = g.find_object(jet).unwrap();
    assert!(j.ai.machine.goal_path.is_empty(), "privateGuardArea cleared the old path (AIUpdate.cpp:4169)");
}

/// The jet template of lane 185's tests: a jet that needs no runway and carries no takeoff pause.
fn jet_template() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Jet");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t.jet = Some(JetAiTemplate {
        needs_runway: false,
        keeps_parking_space_when_airborne: false,
        takeoff_pause: 0,
        return_to_base_idle_time: 0,
        out_of_ammo_damage_per_second: 0.0,
        takeoff_dist_for_max_lift: 0.0,
        min_height: 0.0,
        parking_offset: 0.0,
        sneaky_offset_when_attacking: 0.0,
        attacking_loco: LocomotorSet::Normal,
        returning_loco: LocomotorSet::Normal,
        attack_loco_persist_time: 0,
        attackers_miss_persist_time: 0,
    });
    Arc::new(t)
}

/// A ground vehicle for the OCL tests (a transport or a payload item).
fn vehicle(name: &str) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    Arc::new(t)
}

/// A `DeliverPayload` OCL: a transport `Plane`, the payload `(template, count)`, and a `DelayDeliveryMax`.
fn delivery(payload: Vec<(Arc<ObjectTemplate>, u32)>, delay_delivery_frames_max: u32) -> Arc<crate::ocl::OclTemplate> {
    Arc::new(crate::ocl::OclTemplate {
        name: "Delivery".into(),
        nuggets: vec![crate::ocl::OclNugget::DeliverPayload(Arc::new(crate::ocl::DeliverPayloadTemplate {
            transport: Some(vehicle("Plane")),
            payload,
            delivery_distance: 300.0,
            drop_delay: 5,
            delay_delivery_frames_max,
            drop_offset: (0.0, 0.0, 0.0),
            drop_variance: (0.0, 0.0, 0.0),
            is_parachute_directly: false,
            self_destruct_object: false,
            fire_weapon: false,
            strafing_weapon_slot: 0,
            strafe_length: 0.0,
        }))],
    })
}

/// The delivery world: a 200-cell flat map (its extent is 0..2000 units), a source at (300,1000) and the target at (900,1000).
fn delivery_world() -> (GameLogic, crate::object::ObjectId) {
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    let source = g.create_object(&vehicle("Source"), Coord3D::new(300.0, 1000.0, 0.0), 0.0, 0);
    (g, source)
}

fn plane_of(g: &GameLogic) -> Option<crate::object::ObjectId> {
    g.objects.values().find(|o| o.template.name == "Plane").map(|o| o.id)
}

/// R25-6 (ObjectCreationList.cpp:104-106, OCLSpecialPower.cpp:237-244): an object-based OCL has no `CreateLocation`, so
/// its primary point is the source object's position. The transport starts `DeliveryDistance * 1.5` back from the source
/// (x = 300 - 450 = -150), not at the nearest map edge (x = -450, the special power's default). Revert check: resolve
/// `CreateLocation::default()` for `None` and the start moves to the edge, failing the x assert.
#[test]
fn an_object_based_delivery_starts_back_from_the_source_not_the_nearest_edge() {
    let (mut g, source) = delivery_world();
    let ocl = delivery(Vec::new(), 0);
    g.run_ocl_at_angle(&ocl, source, None, false, None, None);
    let plane = plane_of(&g).expect("the transport was created");
    let x = g.find_object(plane).unwrap().position.x;
    assert!((x - (300.0 - 1.5 * 300.0)).abs() < 0.01, "start x {x}");
}

/// R25-11 (DeliverPayloadAIUpdate.cpp:1168, :145-155): once the exit is ordered the transport refuses AI commands, so a
/// later move does not replace the exit goal. Revert check: drop the `delivery_refuses_commands` gate in
/// `ai_move_to_position_cmd` and the new goal (100,100) replaces the exit.
#[test]
fn a_transport_whose_exit_is_ordered_refuses_a_later_move() {
    let (mut g, source) = delivery_world();
    let ocl = delivery(Vec::new(), 0);
    g.run_ocl_at_angle(&ocl, source, Some(Coord3D::new(900.0, 1000.0, 0.0)), false, None, Some(crate::ocl::CreateLocation::AtLocation));
    let plane = plane_of(&g).unwrap();
    for _ in 0..300 {
        g.run_frames(1);
        if g.deliveries.iter().any(|d| d.exit_ordered) {
            break;
        }
    }
    assert!(g.deliveries.iter().any(|d| d.exit_ordered), "the exit was ordered");
    let exit_goal = g.find_object(plane).unwrap().ai.goal();
    g.ai_move_to_position_cmd(plane, Coord3D::new(100.0, 100.0, 0.0), crate::ai_states::CommandSource::Player);
    assert_eq!(g.find_object(plane).unwrap().ai.goal(), exit_goal, "the transport ignored the later move");
}

/// R25-7 (DeliverPayloadAIUpdate.cpp:700-710, :516-520, :1125-1168): the exit is ordered when the payload is out, not
/// when the approach ends. With one item the item drops `DeliveryDistance` from the target, and the exit follows after
/// `DropDelay`, while the transport is still flying in. Revert check: gate the exit on `!is_ai_moving()` and the exit
/// waits until the transport is at the target.
#[test]
fn the_exit_is_ordered_when_the_payload_is_out_not_when_the_approach_ends() {
    let (mut g, source) = delivery_world();
    let ocl = delivery(vec![(vehicle("Item"), 1)], 0);
    g.run_ocl_at_angle(&ocl, source, Some(Coord3D::new(900.0, 1000.0, 0.0)), false, None, Some(crate::ocl::CreateLocation::AtLocation));
    let plane = plane_of(&g).unwrap();
    for _ in 0..600 {
        g.run_frames(1);
        if g.deliveries.iter().any(|d| d.exit_ordered) {
            break;
        }
    }
    assert!(g.deliveries.iter().any(|d| d.exit_ordered), "the exit was ordered");
    let p = g.find_object(plane).unwrap().position;
    assert!(p.dist_2d(&Coord3D::new(900.0, 1000.0, 0.0)) > 50.0, "the transport is still flying in at {:?}", p);
}

/// R25-8 (DeliverPayloadAIUpdate.cpp:1177-1181, :1219-1229, :381-389): the transport is removed once it leaves the map
/// including its border, not after a timer. It is outside `extent_including_border` when it goes. Revert check: remove
/// the off-map removal and the transport survives the run.
#[test]
fn a_transport_is_removed_when_it_leaves_the_bordered_map() {
    let (mut g, source) = delivery_world();
    let ocl = delivery(Vec::new(), 0);
    g.run_ocl_at_angle(&ocl, source, Some(Coord3D::new(900.0, 1000.0, 0.0)), false, None, Some(crate::ocl::CreateLocation::AtLocation));
    let plane = plane_of(&g).unwrap();
    // Run until the exit is ordered (the payload is out), then let the exit's own motion (the same-ID temporary `AI_MOVE_TO`,
    // `ai_set_temporary_state`) carry the transport off the map. It is kept while it is inside the bordered extent and is
    // removed on the frame it first lies outside it (DeliverPayloadAIUpdate.cpp:1177-1181).
    for _ in 0..600 {
        g.run_frames(1);
        if g.deliveries.iter().any(|d| d.exit_ordered) {
            break;
        }
    }
    assert!(g.deliveries.iter().any(|d| d.exit_ordered), "the exit was ordered");
    let (lo, hi) = g.terrain.extent_including_border();
    let mut removed_at = None;
    for _ in 0..3000 {
        g.run_frames(1);
        match g.find_object(plane) {
            Some(o) => assert!(
                o.position.x >= lo.x && o.position.x <= hi.x && o.position.y >= lo.y && o.position.y <= hi.y,
                "a transport outside the bordered extent is kept only until it leaves: {:?}",
                o.position
            ),
            None => {
                removed_at = Some(g.frame);
                break;
            }
        }
    }
    assert!(removed_at.is_some(), "the exit carried the transport off the bordered map and it was removed");
}

/// R25-10 (ObjectCreationList.cpp:388-390): `DelayDeliveryMax` disables the transport for `GameLogicRandomValue(0, max)`
/// frames. With max 1000 the delay is almost surely positive for the fixed seed. Revert check: remove the
/// `set_disabled_until` call and the transport is never disabled.
#[test]
fn a_delayed_delivery_disables_its_transport_for_a_drawn_number_of_frames() {
    let (mut g, source) = delivery_world();
    let ocl = delivery(Vec::new(), 1000);
    g.run_ocl_at_angle(&ocl, source, Some(Coord3D::new(900.0, 1000.0, 0.0)), false, None, Some(crate::ocl::CreateLocation::AtLocation));
    let plane = plane_of(&g).unwrap();
    let until = g.find_object(plane).unwrap().disabled_till_frame[crate::disabled::DisabledType::Default as usize];
    assert!(until > 0 && until <= 1000, "the transport is disabled until frame {until}");
}
