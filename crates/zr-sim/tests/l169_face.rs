//! Lane 169 (wave 20): `SpecialAbilityUpdate::startFacing` and `AIFaceState` (`SpecialAbilityUpdate.cpp:1899-1926`,
//! `AIStates.cpp:7459-7520`). AH7 seed 1: GLA rebel 240's capture of the Tech Oil Derrick enters `AI_FACE_OBJECT`
//! for one frame and `AI_BUSY` for the unpack; the port marked the facing done at once and never went busy.
//! Synthetic data only.

use std::collections::BTreeMap;
use std::sync::Arc;
use zr_sim::ai::LocoGoalType;
use zr_sim::ai_states::AiStateType;
use zr_sim::physics::PhysicsTemplate;
use zr_sim::special_power::{PackingState, SimSpecialPowerTemplate, SpecialAbilityTemplate, SpecialPowerTarget};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

const POWER: &str = "Capture";

fn world() -> GameLogic {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    // Players 0 and 1 with their default teams: a capture's colleague check compares team instances (F6).
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.scripts_mut().set_player_name("p0", 0);
    g.scripts_mut().set_player_name("p1", 1);
    g.scripts_mut().load_default_team_protos(&[("teamP0", "p0"), ("teamP1", "p1")]);
    g.init_from_sides(&[("teamP0", 0), ("teamP1", 1)]);
    g
}

/// Installs the capture power (no recharge, so the order is accepted at once).
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

/// A capturable building: an immobile structure with a body.
fn building(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::SELECTABLE;
    t.geometry.major_radius = 10.0;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t
}

/// An infantry capture unit: the `SpecialAbility` power module plus a `SpecialAbilityUpdate` with the
/// given unpack / preparation frames and `NeedToFaceTarget` (the engine default, `TRUE`).
fn capture_unit(name: &str, loco: LocomotorParams, unpack: u32, prep: u32) -> ObjectTemplate {
    let module = SpecialAbilityTemplate {
        power: POWER.to_string(),
        start_ability_range: 10_000_000.0,
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
        need_to_face: true,
        flee_range_after_completion: 0.0,
        starts_paused: false,
        power_module: true,
    };
    let update = SpecialAbilityTemplate {
        start_ability_range: 200.0,
        preparation_frames: prep,
        unpack_time: unpack,
        power_module: false,
        ..module.clone()
    };
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 4.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
    t.locomotor = Some(loco);
    t.special_abilities = vec![Arc::new(module), Arc::new(update)];
    t
}

/// The engine's `MinSpeed = 0` infantry locomotor: 120 deg/s of turn, so 0.0698 rad per frame.
fn infantry_loco() -> LocomotorParams {
    LocomotorParams::from_ini(LocomotorKind::Legs, 30.0, 60.0, 120.0, 120.0)
}

fn capture_order(a: zr_sim::ObjectId, b: zr_sim::ObjectId) -> Command {
    Command::DoSpecialPower { source: a, power: POWER.into(), target: SpecialPowerTarget::Object(b) }
}

/// `SpecialAbilityUpdate::startFacing` (`SpecialAbilityUpdate.cpp:1899-1926`) -> `aiFaceObject` ->
/// `privateFaceObject` (`AIUpdate.cpp:2955-2974`): the frame the unit gets in range it enters
/// `AI_FACE_OBJECT` (visible at that frame's end, because the ability module runs after the AI module in
/// the heap), and the next frame `AIFaceState::update` gives the locomotor an angle goal at the bearing
/// to the target, instead of marking the facing done at once.
#[test]
fn a_capturing_unit_enters_ai_face_object_and_gets_an_angle_goal() {
    let mut g = world();
    install_capture(&mut g);
    // Facing +x, target behind-left: a real turn is needed.
    let a = g.create_object(&Arc::new(capture_unit("Rebel", infantry_loco(), 0, 0)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(building("Derrick")), Coord3D::new(160.0, 230.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, capture_order(a, b));
    // Frame 0 dispatches the order; the ability module starts facing on the next frame.
    let mut saw_face_object = false;
    for _ in 0..4 {
        g.run_frames(1);
        if g.find_object(a).unwrap().ai.machine.current == AiStateType::FaceObject {
            saw_face_object = true;
            break;
        }
    }
    assert!(saw_face_object, "startFacing puts the AI in AI_FACE_OBJECT");
    g.run_frames(1);
    let o = g.find_object(a).unwrap();
    assert_eq!(o.ai.locomotor_goal_type, LocoGoalType::Angle, "a turn-in-place locomotor gets an angle goal");
    // `setLocomotorGoalOrientation(obj->getOrientation() + relAngle)`: the goal is the bearing to the
    // target from where the unit stands.
    let (me, target) = (o.position, g.find_object(b).unwrap().position);
    let bearing = zr_sim::math::atan2(target.y - me.y, target.x - me.x);
    assert!(
        zr_sim::math::angle_diff(o.ai.locomotor_goal_data.x, bearing).abs() < 1e-3,
        "the goal orientation is the bearing to the target: goal {} bearing {bearing}",
        o.ai.locomotor_goal_data.x
    );
}

/// `SpecialAbilityUpdate::isFacing` (`:1846-1876`) only guards the *start* of the facing: the frame
/// after `startFacing` the AI is still in `AI_FACE_OBJECT` (not idle), so `needToUnpack` runs
/// (`ai->aiBusy`, `:806-810`) while the locomotor keeps turning the body to the goal orientation.
#[test]
fn the_body_keeps_turning_while_the_unpack_runs_in_ai_busy() {
    let mut g = world();
    install_capture(&mut g);
    let a = g.create_object(&Arc::new(capture_unit("Rebel", infantry_loco(), 90, 0)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(building("Derrick")), Coord3D::new(160.0, 230.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, capture_order(a, b));
    let mut saw_face_object = false;
    for _ in 0..4 {
        g.run_frames(1);
        if g.find_object(a).unwrap().ai.machine.current == AiStateType::FaceObject {
            saw_face_object = true;
            break;
        }
    }
    assert!(saw_face_object, "startFacing puts the AI in AI_FACE_OBJECT");
    g.run_frames(1);
    let o = g.find_object(a).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::Busy, "the unpack runs in AI_BUSY");
    assert_eq!(o.ability.as_ref().unwrap().packing, PackingState::Unpacking, "the unpack animation");
    let start = o.angle;
    g.run_frames(3);
    let o = g.find_object(a).unwrap();
    assert!(o.angle > start, "the body turns toward the target while busy: {start} -> {}", o.angle);
    assert_eq!(o.ai.machine.current, AiStateType::Busy, "AI_BUSY never leaves on its own");
}

/// `AIFaceState::update` succeeds within 0.035 rad of the bearing (`AIStates.cpp:7500-7506`), and the
/// turn is the locomotor's: 2.5 rad at 0.0698 rad per frame takes ~36 frames. The unpack (20 frames)
/// therefore ends before the body has finished turning, and the building is still captured.
#[test]
fn the_capture_completes_after_the_turn_and_the_unpack() {
    let mut g = world();
    install_capture(&mut g);
    let a = g.create_object(&Arc::new(capture_unit("Rebel", infantry_loco(), 20, 0)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(building("Derrick")), Coord3D::new(160.0, 230.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, capture_order(a, b));
    g.run_frames(120);
    assert_eq!(g.find_object(b).unwrap().player, 0, "captured");
    assert!(g.find_object(a).unwrap().angle > 0.5, "the body turned toward the target");
}

/// `startUnpacking` -> `ai->aiBusy(CMD_FROM_AI)` (`SpecialAbilityUpdate.cpp:806-810`): the unpack
/// animation runs in `AI_BUSY`, which never leaves on its own.
#[test]
fn the_unpack_animation_puts_the_unit_in_ai_busy() {
    let mut g = world();
    install_capture(&mut g);
    // Facing the target already, so the face succeeds at its first update.
    let a = g.create_object(&Arc::new(capture_unit("Rebel", infantry_loco(), 90, 0)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(building("Derrick")), Coord3D::new(240.0, 200.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, capture_order(a, b));
    g.run_frames(6);
    let o = g.find_object(a).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::Busy, "unpacking runs in AI_BUSY");
    assert!(o.ability.is_some(), "the capture is still unpacking");
    g.run_frames(40);
    assert_eq!(g.find_object(a).unwrap().ai.machine.current, AiStateType::Busy, "still busy 40 frames in");
}
