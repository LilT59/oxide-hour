//! `AISkirmishPlayer::buildAIBaseDefenseStructure` (AISkirmishPlayer.cpp:599-709) requests each defense at
//! `buildPos = m_baseCenter` (AISkirmishPlayer.cpp:688), and `m_baseCenter.z` is 0 (`AIPlayer::m_baseCenter.zero()`,
//! AIPlayer.cpp:97; `computeCenterAndRadiusOfBase` sets only x and y). The ground height is added later, in
//! `buildStructureWithDozer` (AIPlayer.cpp:530). The legality test in between uses the z interval of
//! `geomCollidesWithGeom` (PartitionManager.cpp:2065), so a standing structure on higher ground does not block
//! a request that is still at z 0. AN3 s1 (f2829): the port tested the American FireBase site at the ground height
//! (18.75), so a civilian `GermanCotage01` on that ground blocked the site and the defense landed 42 cells off.
//! Synthetic data only (no game install).

use std::sync::Arc;

use zr_sim::script::ScriptEngine;
use zr_sim::skirmish_ai::{AiBuildOrder, SkirmishAiConfig};
use zr_sim::template::{kind_of, GeometryShape};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TemplateSource, TerrainLogic};

struct FixedStore(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for FixedStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

/// A box of half extent `half` and height `height`, immobile, with no health to speak of.
fn structure(name: &str, half: f32, height: f32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    t.max_health = 500.0;
    t.initial_health = 500.0;
    t.geometry.shape = GeometryShape::Box;
    t.geometry.major_radius = half;
    t.geometry.minor_radius = half;
    t.geometry.height = height;
    t
}

/// Flat ground at `TerrainLogic` sample 32, which is 32 * MAP_HEIGHT_SCALE (10 / 16) = 20 units.
const GROUND: f32 = 20.0;

/// Player 0's base centre is the command centre at (100, 100) (the only build-list entry), and the base-defense
/// margin is 40 (`SkirmishBaseDefenseExtraDistance`). With no enemy and no `Center1` path the goal falls back to
/// the origin, so the front defense #1 (slot 0, the left of the fan) sits on the ray from (100, 100) towards it.
fn world(blocker: Option<Coord3D>) -> GameLogic {
    let templates = [
        structure("AmericaCommandCenter", 15.0, 20.0),
        structure("AmericaFireBase", 26.0, 10.0),
        structure("CivilianBuilding", 20.0, 10.0),
    ];
    let store = FixedStore(templates.iter().cloned().map(Arc::new).collect());
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 32), 1);
    g.add_player(0, 1000);
    g.add_player(1, 0);
    g.set_template_source(Arc::new(store));
    let cfg = SkirmishAiConfig {
        build_list: vec![AiBuildOrder {
            template: "AmericaCommandCenter".into(),
            location: (100.0, 100.0),
            angle: 0.0,
            initially_built: false,
            rebuilds: 1,
            automatically_build: true,
        }],
        base_defense_extra_distance: 40.0,
        ..Default::default()
    };
    g.add_ai_player(0, cfg);
    if let Some(at) = blocker {
        let civilian = Arc::new(templates[2].clone());
        g.create_object(&civilian, at, 0.0, 1);
    }
    g
}

/// Requests the front defense (`AIPlayer::buildAIBaseDefenseStructure` with `flank` false) and returns the
/// position of the priority build-list entry it added.
fn requested_front_defense(g: &mut GameLogic) -> (f32, f32) {
    g.ai_build_base_structure(0, "AmericaFireBase", false, &ScriptEngine::default());
    let ai = g.ai_player(0).expect("player 0 is an AI");
    let first = &ai.config.build_list[0];
    assert_eq!(first.template, "AmericaFireBase", "the defense is prepended as a priority entry");
    first.location
}

/// The slot-0 site of the unblocked map: at the base radius plus the margin, on the ray towards the origin.
fn expected_slot_zero(g: &GameLogic) -> (f32, f32) {
    let ai = g.ai_player(0).expect("player 0 is an AI");
    let d = ai.base_radius + 40.0;
    let (ux, uy) = (-100.0f32, -100.0f32);
    let len = (ux * ux + uy * uy).sqrt();
    (100.0 + ux / len * d, 100.0 + uy / len * d)
}

/// On flat ground with nothing in the way, the first front defense takes slot 0 (AISkirmishPlayer.cpp:665-672:
/// the even selector takes `m_curFrontLeftDefenseAngle`, which starts at 0).
#[test]
fn a_front_defense_takes_slot_zero_on_open_ground() {
    let mut g = world(None);
    let site = requested_front_defense(&mut g);
    let want = expected_slot_zero(&g);
    assert!((site.0 - want.0).abs() < 0.01 && (site.1 - want.1).abs() < 0.01, "site {site:?}, slot 0 {want:?}");
}

/// A civilian structure whose base lies above the defense's top does not block the request
/// (`geomCollidesWithGeom`'s z interval, PartitionManager.cpp:2065-2066), because the request is at z 0
/// (`m_baseCenter`), not at the ground height. The structure here stands at ground 20 with height 10,
/// the defense's height is 10, so the z intervals [0, 10] and [20, 30] are disjoint. The defense takes slot 0.
/// Requesting the site at the ground height instead (the pre-fix port) makes the intervals overlap, the slot-0
/// site is refused and the defense takes slot 1 on the other side of the fan. Revert check: that fails this test.
#[test]
fn a_structure_on_higher_ground_does_not_block_an_unbuilt_front_defense() {
    let open = {
        let mut g = world(None);
        let site = requested_front_defense(&mut g);
        let want = expected_slot_zero(&g);
        assert!((site.0 - want.0).abs() < 0.01 && (site.1 - want.1).abs() < 0.01);
        site
    };
    let mut g = world(Some(Coord3D::new(open.0, open.1, GROUND)));
    let site = requested_front_defense(&mut g);
    assert!(
        (site.0 - open.0).abs() < 0.01 && (site.1 - open.1).abs() < 0.01,
        "a structure at z {GROUND} (base {GROUND}..{}) must not block a defense requested at z 0: site {site:?}, expected {open:?}",
        GROUND + 10.0
    );
}
