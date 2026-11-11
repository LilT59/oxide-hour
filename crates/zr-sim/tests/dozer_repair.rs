//! Dozer/worker repair, resume-construction, and idle auto-repair
//! (`DozerAIUpdate` / `WorkerAIUpdate` / `ActionManager`). Synthetic data, no game install.

use std::sync::Arc;

use zr_sim::ai_states::AiStateType;
use zr_sim::dozer::{DozerActionSub, DozerTemplate};
use zr_sim::supply::SupplyTruckTemplate;
use zr_sim::template::{kind_of, GeometryShape, LocomotorKind, LocomotorParams};
use zr_sim::{
    Command, Coord3D, DamageInfo, DamageType, GameLogic, ObjectId, ObjectTemplate, TemplateSource, TerrainLogic,
};

struct FixedStore(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for FixedStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

fn mobile(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 5.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
    t
}

/// A dozer/worker: `KINDOF_DOZER` plus the `DozerAIUpdate` fields.
fn dozer(name: &str, rate: f32, bored_time: f32, bored_range: f32) -> ObjectTemplate {
    let mut t = mobile(name);
    t.kind_of |= kind_of::DOZER;
    t.dozer = Some(DozerTemplate {
        repair_health_percent_per_second: rate,
        bored_time_frames: bored_time,
        bored_range,
    });
    t
}

fn structure(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    t.max_health = 500.0;
    t.initial_health = 500.0;
    t.geometry.shape = GeometryShape::Box;
    t.geometry.major_radius = 15.0;
    t.geometry.minor_radius = 15.0;
    t.build_cost = 300;
    t.build_time_frames = 50;
    t
}

fn world(templates: Vec<ObjectTemplate>, money: u32) -> GameLogic {
    let store = FixedStore(templates.into_iter().map(Arc::new).collect());
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, money);
    g.set_template_source(Arc::new(store));
    g
}

fn damage(g: &mut GameLogic, id: ObjectId, amount: f32) {
    g.damage_object(id, DamageInfo { source: ObjectId::INVALID, amount, kind: DamageType::Unresistable });
}

/// `ActionManager::canRepairObject`: damaged friendly structure yes; full health and non-dozers no.
#[test]
fn can_repair_object_checks_are_faithful() {
    let mut g = world(vec![dozer("Dozer", 0.1, 1.0, 100.0), mobile("Tank"), structure("Plant")], 1000);
    let d = g.create_object(&g.find_template("Dozer").unwrap(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let tank = g.create_object(&g.find_template("Tank").unwrap(), Coord3D::new(100.0, 140.0, 0.0), 0.0, 0);
    let plant = g.create_object(&g.find_template("Plant").unwrap(), Coord3D::new(160.0, 100.0, 0.0), 0.0, 0);
    assert!(!g.can_repair_object(d, plant), "full health is rejected");
    damage(&mut g, plant, 200.0);
    assert!(g.can_repair_object(d, plant), "a damaged friendly structure can be repaired");
    assert!(!g.can_repair_object(tank, plant), "only dozers repair");
}

/// `Command::Repair` heals by `RepairHealthPercentPerSecond` per second and clears at full health.
#[test]
fn dozer_repairs_a_damaged_structure() {
    // `bored_range` 0 so the healing comes from the `Command::Repair` path, not the idle auto-repair.
    let mut g = world(vec![dozer("Dozer", 0.1, 1.0, 0.0), structure("Plant")], 1000);
    let d = g.create_object(&g.find_template("Dozer").unwrap(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let plant = g.create_object(&g.find_template("Plant").unwrap(), Coord3D::new(160.0, 100.0, 0.0), 0.0, 0);
    damage(&mut g, plant, 300.0);
    assert!(g.find_object(plant).unwrap().health() < 250.0);

    g.submit(0, Command::Repair { dozer: d, target: plant });
    // 10% of 500 max hp per second = 50 hp/s.
    g.run_frames(30);
    let hp = g.find_object(plant).unwrap().health();
    assert!(hp > 200.0, "a second of repair added ~50 hp, got {hp}");
    assert_eq!(g.find_object(d).unwrap().repair_target, Some(plant), "the dozer is marked repairing");
    g.run_frames(300);
    assert_eq!(g.find_object(plant).unwrap().health(), 500.0, "repaired to full");
    assert_eq!(g.find_object(d).unwrap().repair_target, None, "task cleared at full health");
}

/// `DozerPrimaryIdleState`: an idle dozer auto-repairs the closest damaged friendly structure
/// within `BoredRange` after `BoredTime`.
#[test]
fn idle_dozer_auto_repairs_within_bored_range() {
    let mut g = world(vec![dozer("Dozer", 0.2, 1.0, 100.0), structure("Plant")], 1000);
    let d = g.create_object(&g.find_template("Dozer").unwrap(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let plant = g.create_object(&g.find_template("Plant").unwrap(), Coord3D::new(160.0, 100.0, 0.0), 0.0, 0);
    damage(&mut g, plant, 400.0);
    g.run_frames(60);
    assert!(g.find_object(plant).unwrap().health() > 200.0, "the bored dozer healed the plant");
    assert!(g.find_object(d).unwrap().repair_target.is_some() || g.find_object(plant).unwrap().health() == 500.0);
}

/// A dozer outside `BoredRange` does not wander off to repair.
#[test]
fn idle_dozer_outside_bored_range_stays_put() {
    let mut g = world(vec![dozer("Dozer", 0.2, 1.0, 50.0), structure("Plant")], 1000);
    let d = g.create_object(&g.find_template("Dozer").unwrap(), Coord3D::new(40.0, 40.0, 0.0), 0.0, 0);
    let plant = g.create_object(&g.find_template("Plant").unwrap(), Coord3D::new(340.0, 340.0, 0.0), 0.0, 0);
    damage(&mut g, plant, 400.0);
    g.run_frames(120);
    assert!(g.find_object(d).unwrap().repair_target.is_none(), "too far to care");
    assert!(g.find_object(plant).unwrap().health() < 200.0);
}

/// Kill the dozer mid-construction, then another dozer resumes the unfinished structure
/// (`ActionManager::canResumeConstructionOf` -> `MSG_RESUME_CONSTRUCTION`).
#[test]
fn kill_dozer_mid_construction_and_resume_with_another() {
    let mut g = world(vec![dozer("Dozer", 0.1, 1.0, 100.0), structure("Barracks")], 1000);
    let a = g.create_object(&g.find_template("Dozer").unwrap(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&g.find_template("Dozer").unwrap(), Coord3D::new(100.0, 120.0, 0.0), 0.0, 0);
    let site = Coord3D::new(220.0, 200.0, 0.0);
    let bldg = g.construct(a, "Barracks", site, 0.0).expect("construct");
    // Only the original builder may keep it; a second dozer cannot double up.
    assert!(!g.can_resume_construction(b, bldg), "the live builder blocks a second dozer");
    assert_eq!(g.find_object(bldg).unwrap().builder, Some(a));

    // Kill the builder before it finishes.
    g.destroy_object(a);
    g.update();
    assert!(g.find_object(bldg).unwrap().under_construction, "the scaffold survives the builder");

    // Another dozer resumes it.
    assert!(g.can_resume_construction(b, bldg), "a dead builder frees the scaffold");
    g.submit(0, Command::ResumeConstruction { dozer: b, target: bldg });
    g.run_frames(240);
    let bldg_obj = g.find_object(bldg).unwrap();
    assert!(!bldg_obj.under_construction, "resumed construction completed");
    assert_eq!(bldg_obj.construction_percent, 100.0);
    assert_eq!(g.find_object(b).unwrap().build_target, None);
}

/// `DozerPrimaryIdleState::update` refreshes `m_idleTooLongTimestamp` on every frame
/// `AIUpdateInterface::isIdle` is false (`DozerAIUpdate.cpp:1112-1113`), whether or not a task is
/// pending: a task that arrives while the AI is busy leaves the primary machine in `DOZER_PRIMARY_IDLE`
/// until `isBuildMostImportant` sees an idle AI (`:1292-1301`, `:1346-1366`), and the idle state's update
/// still runs on those frames. The bored search itself needs `isAnyTaskPending() == FALSE`
/// (`:1117-1120`). Revert check: with the old `busy` early return the timestamp stays at the task frame.
#[test]
fn a_pending_build_task_does_not_freeze_the_bored_timestamp() {
    // `BoredTime` 1000 so no search can fire; `BoredRange` 0 so no repair is in reach either.
    let mut g = world(vec![dozer("Dozer", 0.1, 1000.0, 0.0), structure("Barracks")], 1000);
    let d = g.create_object(&g.find_template("Dozer").unwrap(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    // Busy first: a long move, then the build task in the next frame.
    g.submit(0, Command::MoveTo { ids: vec![d], pos: Coord3D::new(390.0, 390.0, 0.0) });
    g.run_frames(1);
    let b = g.construct(d, "Barracks", Coord3D::new(330.0, 100.0, 0.0), 0.0).expect("construct");
    let task_frame = g.frame();
    assert_eq!(g.find_object(d).unwrap().ai.dozer_action.sub, DozerActionSub::Inactive, "BUILD waits for an idle AI");
    assert_eq!(g.find_object(d).unwrap().build_target, Some(b));
    g.run_frames(30);
    let o = g.find_object(d).unwrap();
    assert_ne!(o.ai.machine.current, AiStateType::Idle, "the dozer is still moving");
    assert_eq!(o.ai.dozer_action.sub, DozerActionSub::Inactive, "still in DOZER_PRIMARY_IDLE");
    assert!(
        o.dozer_idle_frame > task_frame,
        "the idle state refreshed the timestamp while the AI was busy: {} vs the task frame {task_frame}",
        o.dozer_idle_frame
    );
}

/// `DozerPrimaryIdleState::onEnter` sets `m_idleTooLongTimestamp = TheGameLogic->getFrame()`
/// (`DozerAIUpdate.cpp:1042-1051`), so the dozer waits `BoredTime` again after a repair task ends: while
/// the primary machine is in `DOZER_PRIMARY_REPAIR` the idle state's update does not run, so the
/// timestamp is as old as the arrival. Revert check: without the IDLE re-entry reset it still holds the
/// arrival frame (~150 frames earlier).
#[test]
fn finishing_a_repair_resets_the_bored_timestamp() {
    let mut g = world(vec![dozer("Dozer", 0.1, 1000.0, 0.0), structure("Plant")], 1000);
    let d = g.create_object(&g.find_template("Dozer").unwrap(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let plant = g.create_object(&g.find_template("Plant").unwrap(), Coord3D::new(160.0, 100.0, 0.0), 0.0, 0);
    damage(&mut g, plant, 300.0);
    g.submit(0, Command::Repair { dozer: d, target: plant });
    let mut end = None;
    for _ in 0..1000 {
        g.run_frames(1);
        if g.find_object(d).unwrap().repair_target.is_none() {
            end = Some(g.frame());
            break;
        }
    }
    let end = end.expect("the repair completed");
    let o = g.find_object(d).unwrap();
    assert_eq!(o.repair_target, None);
    assert!(
        end.saturating_sub(o.dozer_idle_frame) <= 1,
        "IDLE re-entry resets the timestamp: {} vs the end frame {end}",
        o.dozer_idle_frame
    );
}

/// A GLA worker is a dozer plus a supply truck (`WorkerAIUpdate`): it repairs like a dozer.
#[test]
fn gla_worker_repairs() {
    let mut worker = dozer("GLAWorker", 0.1, 1.0, 100.0);
    worker.kind_of |= kind_of::HARVESTER;
    worker.supply_truck = Some(SupplyTruckTemplate { max_boxes: 1, center_action_delay: 1, warehouse_action_delay: 1, scan_distance: 100.0 });
    let mut g = world(vec![worker, structure("Palace")], 1000);
    let w = g.create_object(&g.find_template("GLAWorker").unwrap(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let palace = g.create_object(&g.find_template("Palace").unwrap(), Coord3D::new(160.0, 100.0, 0.0), 0.0, 0);
    damage(&mut g, palace, 300.0);
    assert!(g.can_repair_object(w, palace));
    g.submit(0, Command::Repair { dozer: w, target: palace });
    g.run_frames(240);
    assert_eq!(g.find_object(palace).unwrap().health(), 500.0, "the worker repaired the palace");
}

/// An AI player's idle dozer repairs its damaged base (`AIDozerBoredRadiusModifier` widens the scan).
#[test]
fn ai_player_repairs_its_damaged_base() {
    let mut g = world(vec![dozer("Dozer", 0.2, 1.0, 50.0), structure("Plant")], 1000);
    let d = g.create_object(&g.find_template("Dozer").unwrap(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let plant = g.create_object(&g.find_template("Plant").unwrap(), Coord3D::new(180.0, 100.0, 0.0), 0.0, 0);
    damage(&mut g, plant, 400.0);
    // 80 units apart, outside the human BoredRange (50) but inside the AI's 2x scan (100).
    g.add_ai_player(0, empty_ai_config());
    g.run_frames(90);
    assert!(g.find_object(plant).unwrap().health() > 150.0, "the AI dozer healed its base");
    let _ = d;
}

/// Minimal AI config with no structures to build, so only the idle auto-repair runs.
fn empty_ai_config() -> zr_sim::skirmish_ai::SkirmishAiConfig {
    zr_sim::skirmish_ai::SkirmishAiConfig {
        build_list: Vec::new(),
        resource_gatherers: 0,
        base_defense: None,
        gatherer_template: String::new(),
        gatherer_factory: String::new(),
        attack_units: Vec::new(),
        attack_factory: String::new(),
        structure_seconds: 0,
        team_seconds: 60,
        rebuild_delay_seconds: 30,
        resources_wealthy: 2000,
        resources_poor: 500,
        structures_wealthy_mod: 1.0,
        structures_poor_mod: 1.0,
        team_wealthy_mod: 1.0,
        team_poor_mod: 1.0,
        team_resources_to_build: 1.0,
        supply_center_safe_radius: 250.0,
        max_recruit_distance: 500.0,
        skill_sets: Vec::new(),
        base_defense_extra_distance: 0.0,
        difficulty: zr_sim::script::Difficulty::Normal,
    }
}
