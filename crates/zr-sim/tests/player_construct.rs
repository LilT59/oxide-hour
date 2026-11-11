//! A human player's `Command::Construct` sends a distant dozer to the site, and the site builds to 100%.
//! Synthetic data only (no game install).

use std::sync::Arc;

use zr_sim::ai_states::AiStateType;
use zr_sim::command::Command;
use zr_sim::template::{kind_of, GeometryShape, LocomotorKind, LocomotorParams};
use zr_sim::{Coord3D, GameLogic, ObjectId, ObjectTemplate, TemplateSource, TerrainLogic};

struct FixedStore(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for FixedStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

fn dozer_template() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Dozer");
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 5.0;
    t.geometry.minor_radius = 5.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
    t.kind_of |= kind_of::DOZER;
    t.dozer = Some(zr_sim::dozer::DozerTemplate { repair_health_percent_per_second: 0.0, bored_time_frames: 1.0, bored_range: 0.0 });
    t
}

fn barracks() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Barracks");
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    t.max_health = 500.0;
    t.initial_health = 500.0;
    t.geometry.shape = GeometryShape::Box;
    t.geometry.major_radius = 15.0;
    t.geometry.minor_radius = 15.0;
    t.build_cost = 300;
    t.build_time_frames = 60;
    t
}

fn world() -> GameLogic {
    let store = FixedStore(vec![Arc::new(dozer_template()), Arc::new(barracks())]);
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 1000);
    g.set_template_source(Arc::new(store));
    g
}

fn dozer_at(g: &mut GameLogic, at: Coord3D) -> ObjectId {
    let t = g.find_template("Dozer").expect("dozer template");
    g.create_object(&t, at, 0.0, 0)
}

/// The human order path (`Command::Construct` through `submit`) from a dozer 300 units away must walk the
/// dozer to the site and build it to completion.
#[test]
fn distant_dozer_builds_a_player_ordered_site() {
    let mut g = world();
    let d = dozer_at(&mut g, Coord3D::new(100.0, 100.0, 0.0));
    let site = Coord3D::new(400.0, 100.0, 0.0);
    g.submit(0, Command::Construct { dozer: d, template: "Barracks".into(), pos: site, angle: 0.0 });
    g.run_frames(1);
    let b = g
        .objects()
        .find(|o| o.template.name == "Barracks")
        .map(|o| o.id)
        .expect("the order created the building");

    let mut trace = Vec::new();
    for frame in 0..1500 {
        g.run_frames(1);
        let o = g.find_object(b).expect("building");
        if frame % 100 == 0 {
            let dz = g.find_object(d).expect("dozer");
            trace.push(format!(
                "f{frame}: pct {:.1} dozer ({:.0},{:.0}) state {:?} moving {} sub {:?}",
                o.construction_percent,
                dz.position.x,
                dz.position.y,
                dz.ai.machine.current,
                dz.ai.is_moving(),
                dz.ai.dozer_action.sub
            ));
        }
        if !o.under_construction {
            assert_eq!(o.construction_percent, 100.0);
            let dz = g.find_object(d).expect("dozer");
            assert_ne!(dz.ai.machine.current, AiStateType::Dead, "trace: {trace:?}");
            return;
        }
    }
    panic!("site never finished; trace: {trace:#?}");
}
