//! Lane 179 (R21-7): `doStuffToObj`'s `LIKE_EXISTING` structure block re-reads the ground for the object's z
//! at the **base** position: `adjustedPos.z = TheTerrainLogic->getGroundHeight(pos->x, pos->y)`
//! (ObjectCreationList.cpp:1061). `pos` is the parameter; the object itself sits at `chunkPos = pos + offset`
//! (:969-972). Synthetic data only: a ramp terrain and a single structure nugget with an x offset.
//! Revert check: reading the ground at the object's own position (`ocl.rs`, `ground_height(p.x, p.y)`) gives the
//! ground at the offset point and the z check fails.

use std::collections::BTreeMap;
use std::sync::Arc;
use zr_sim::ocl::{disposition, CreateObjectTemplate, OclNugget, OclTemplate};
use zr_sim::special_power::{SimSpecialPower, SimSpecialPowerTemplate, SpecialPowerEffect};
use zr_sim::template::kind_of;
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TemplateSource, TerrainLogic};

struct FixedStore(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for FixedStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

#[test]
fn a_like_existing_structure_takes_its_ground_height_at_the_base_point() {
    // A ramp that rises 5 world units per 10-unit cell in x: the ground at x = 50 and x = 80 differ.
    let mut data = vec![0u8; 32 * 32];
    for y in 0..32 {
        for x in 0..32 {
            data[y * 32 + x] = (x * 8) as u8;
        }
    }
    let terrain = TerrainLogic::from_samples(32, 32, 0, data);
    let base_ground = terrain.ground_height(50.0, 50.0);
    let offset_ground = terrain.ground_height(80.0, 50.0);
    assert!((base_ground - offset_ground).abs() > 1.0, "the ramp slopes between the two points");

    // The structure's footprint (radius 5 about x = 80) does not reach x = 50, so the base ground is not flattened.
    let mut marker = ObjectTemplate::new("Marker");
    marker.kind_of = kind_of::STRUCTURE;
    marker.geometry.major_radius = 5.0;
    marker.geometry.minor_radius = 5.0;
    marker.max_health = 100.0;
    marker.initial_health = 100.0;
    let ocl = Arc::new(OclTemplate {
        name: "BuildOcl".into(),
        nuggets: vec![OclNugget::CreateObject(CreateObjectTemplate {
            is_object: true,
            names_len: 1,
            objects: vec![Some(Arc::new(marker.clone()))],
            count: 1,
            offset: (30.0, 0.0, 0.0),
            disposition: disposition::LIKE_EXISTING,
            ..CreateObjectTemplate::default()
        })],
    });
    let mut source = ObjectTemplate::new("Source");
    source.kind_of = kind_of::STRUCTURE;
    source.max_health = 500.0;
    source.initial_health = 500.0;
    source.special_powers = vec![Arc::new(SimSpecialPower {
        power: "Build".into(),
        effect: SpecialPowerEffect::Ocl { ocl: Some(ocl), create_at_target: true, create_location: zr_sim::ocl::CreateLocation::AtLocation },
        starts_paused: false,
        power_module: true,
    })];

    let mut g = GameLogic::new(terrain, 1);
    g.add_player(0, 0);
    g.set_template_source(Arc::new(FixedStore(vec![Arc::new(marker), Arc::new(source.clone())])));
    let mut powers = BTreeMap::new();
    powers.insert(
        "Build".to_string(),
        Arc::new(SimSpecialPowerTemplate {
            name: "Build".into(),
            kind: String::new(),
            reload_time: 0,
            required_science: None,
            shared_synced_timer: false,
            public_timer: false,
            radius_cursor_radius: 0.0,
        }),
    );
    g.set_special_powers(powers);
    let src = g.create_object(&Arc::new(source), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    g.submit(0, Command::DoSpecialPowerAtLocation { source: src, power: "Build".into(), pos: Coord3D::new(50.0, 50.0, 0.0), angle: None });
    g.run_frames(1);

    let m = g.objects().find(|o| o.template.name == "Marker").expect("the structure is created");
    assert!((m.position.x - 80.0).abs() < 1e-3, "the object sits at base + offset: {:?}", m.position);
    assert!(
        (m.position.z - base_ground).abs() < 1e-3,
        "z is the ground at the base point ({base_ground}), not at the offset point ({offset_ground}): {:?}",
        m.position
    );
}
