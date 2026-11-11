//! Lane 171b: `TerrainLogic::flattenTerrain` (`TerrainLogic.cpp:2619`) through the dozer's build
//! (`DozerAIUpdate.cpp:1730`, `WorkerAIUpdate.cpp:420`). A structure lowers the height samples under
//! its footprint (and one cell around it) to the average ground height of the grid points it covers,
//! then the site's z is re-read from the flattened ground. Synthetic data only.
//!
//! The footprint itself is flat here so the site passes `TERRAIN_RESTRICTIONS`; the averaged lowering
//! of a sloped footprint is `terrain::tests::flatten_terrain_averages_the_footprint_and_stamps_3x3`.

use std::sync::Arc;

use zr_sim::template::{kind_of, GeometryShape, LocomotorKind, LocomotorParams};
use zr_sim::{Coord3D, GameLogic, ObjectId, ObjectTemplate, TemplateSource, TerrainLogic};

struct FixedStore(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for FixedStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

fn barracks() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Barracks");
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    t.max_health = 500.0;
    t.initial_health = 500.0;
    t.geometry.shape = GeometryShape::Box;
    t.geometry.major_radius = 10.0;
    t.geometry.minor_radius = 5.0;
    t.build_cost = 300;
    t.build_time_frames = 10;
    t
}

fn dozer() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Dozer");
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 5.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
    t
}

/// A 40x40-cell map (border 0, padded two cells like `TerrainLogic::flat`): sample 30 everywhere, with
/// a 40 bump at cell (17,19) — one cell outside the footprint at (150, 200) — and another at (20,19),
/// two cells further out.
fn bumps() -> TerrainLogic {
    let (w, h) = (42, 42);
    let mut data = vec![30u8; w * h];
    for (x, y) in [(17usize, 19usize), (20, 19)] {
        data[x + y * w] = 40;
    }
    TerrainLogic::from_samples(w as i32, h as i32, 0, data)
}

fn world() -> GameLogic {
    let store = FixedStore(vec![Arc::new(barracks()), Arc::new(dozer())]);
    let mut g = GameLogic::new(bumps(), 1);
    g.add_player(0, 1000);
    g.set_template_source(Arc::new(store));
    g
}

fn spawn_dozer(g: &mut GameLogic) -> ObjectId {
    let t = g.find_template("Dozer").unwrap();
    g.create_object(&t, Coord3D::new(40.0, 40.0, 0.0), 0.0, 0)
}

/// The footprint at (150, 200) (half extents 10 x 5, angle 0) covers the grid points x = 140/150/160 and
/// y = 190/200, all at sample 30, so its average is 18.75 and the stamp lowers the uncovered bump at
/// (17,19) — one cell to the right of the covered (16,19) — to that height, while (20,19) is out of reach.
#[test]
fn a_dozer_build_flattens_the_ground_beside_the_structure() {
    let mut g = world();
    let d = spawn_dozer(&mut g);
    let at = Coord3D::new(150.0, 200.0, 0.0);
    assert_eq!(g.terrain().ground_height(170.0, 190.0), 25.0, "the bump starts at sample 40");
    let id = g.construct(d, "Barracks", at, 0.0).expect("a legal site");
    let after = g.terrain().ground_height(150.0, 200.0);
    assert_eq!(after, 18.75, "the flat footprint keeps its height");

    // The site's z is re-read from the flattened ground (`adjustedPos.z = getGroundHeight(pos->x, pos->y)`).
    let site = g.find_object(id).expect("the site").position;
    assert_eq!(site.z, after, "the site sits on the flattened ground");

    // The 3x3 stamp reaches one cell outside the footprint: (17,19) neighbours the covered (16,19).
    assert_eq!(g.terrain().ground_height(170.0, 190.0), 18.75, "the cell beside the footprint was stamped");
    // Two cells away is out of reach.
    assert_eq!(g.terrain().ground_height(200.0, 190.0), 25.0, "the stamp does not reach two cells away");
}
