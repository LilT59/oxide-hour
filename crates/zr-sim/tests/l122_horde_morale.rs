//! `AIUpdateInterface::evaluateMoraleBonus` (AIUpdate.cpp:4741-4758, classic
//! `evaluateNationalismBonusClassic` :4790-4824) as `HordeUpdate::update` calls it
//! (HordeUpdate.cpp:299-301): HORDE follows the horde state, NATIONALISM needs `AllowedNationalism` and
//! `Upgrade_Nationalism`, FANATICISM needs NATIONALISM in the classic action. Synthetic data only.

use std::sync::Arc;

use zr_sim::bonus::condition;
use zr_sim::modules2::HordeTemplate;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

/// A vehicle with an `AIUpdate`: `HordeUpdate::update` evaluates the morale bonus only through the object's AI
/// (HordeUpdate.cpp:298-301, `getAIUpdateInterface`; the "must have an AI to benefit from horde" check at :201-208).
fn unit(name: &str, horde: Option<HordeTemplate>) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t.ai_update = Some(1);
    t.horde = horde;
    Arc::new(t)
}

fn horde(count: i32, allowed_nationalism: bool) -> HordeTemplate {
    HordeTemplate {
        update_rate: 1,
        kind_of: kind_of::VEHICLE,
        count,
        radius: 100.0,
        rub_off_radius: 20.0,
        allies_only: true,
        exact_match: false,
        allowed_nationalism,
    }
}

fn has(g: &GameLogic, id: zr_sim::ObjectId, cond: u8) -> bool {
    g.find_object(id).is_some_and(|o| o.combat.weapon_bonus_conditions & (1 << cond) != 0)
}

/// The classic action's documented retail bug: leaving the horde clears HORDE but not NATIONALISM or
/// FANATICISM (`AIUpdate.cpp:4793-4796`, "Nationalism and Fanaticism bonuses are not disabled when
/// leaving the horde"). `AllowedNationalism = No` clears NATIONALISM even while horded.
#[test]
fn morale_bonus_needs_allowed_nationalism_and_follows_the_horde() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    g.add_player(0, 0);
    g.apply_player_upgrade(0, "Upgrade_Nationalism");
    g.apply_player_upgrade(0, "Upgrade_Fanaticism");
    // Count 2 needs one other member: alone, so not in a horde.
    let id = g.create_object(&unit("Redguard", Some(horde(2, true))), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    // Move in a comrade for a while to gain the horde (and its nationalism/fanaticism), then remove it. The comrade
    // needs a HordeUpdate of its own to count (`getHUI`, HordeUpdate.cpp:93-96).
    let friend = g.create_object(&unit("Redguard2", Some(horde(2, true))), Coord3D::new(220.0, 200.0, 0.0), 0.0, 0);
    g.run_frames(3);
    assert!(g.find_object(id).unwrap().behavior.horde, "in a horde");
    assert!(has(&g, id, condition::HORDE), "HORDE with the horde");
    assert!(has(&g, id, condition::NATIONALISM), "NATIONALISM with AllowedNationalism and the upgrade");
    assert!(has(&g, id, condition::FANATICISM), "FANATICISM with NATIONALISM and the upgrade");

    g.destroy_object(friend);
    g.run_frames(3);
    assert!(!g.find_object(id).unwrap().behavior.horde, "alone again");
    assert!(!has(&g, id, condition::HORDE), "HORDE cleared with the horde");
    assert!(has(&g, id, condition::NATIONALISM), "the classic action leaves NATIONALISM set (AIUpdate.cpp:4793-4796)");
    assert!(has(&g, id, condition::FANATICISM), "the classic action leaves FANATICISM set (AIUpdate.cpp:4793-4796)");
}

/// `AllowedNationalism = No` keeps HORDE but never applies the player's upgrades (`HordeUpdate.cpp:299-301`).
#[test]
fn allowed_nationalism_off_keeps_only_the_horde_bonus() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    g.add_player(0, 0);
    g.apply_player_upgrade(0, "Upgrade_Nationalism");
    g.apply_player_upgrade(0, "Upgrade_Fanaticism");
    let t = unit("Redguard", Some(horde(1, false)));
    let id = g.create_object(&t, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.run_frames(3);
    assert!(g.find_object(id).unwrap().behavior.horde, "count 1 is a horde on its own");
    assert!(has(&g, id, condition::HORDE));
    assert!(!has(&g, id, condition::NATIONALISM), "AllowedNationalism = No");
    assert!(!has(&g, id, condition::FANATICISM), "AllowedNationalism = No");
}

/// The player's upgrade alone must not grant NATIONALISM/FANATICISM: the condition comes from the horde
/// evaluation, not from `weapon_bonus_flags`. (This is the lane-116 finding.)
#[test]
fn a_player_upgrade_alone_does_not_set_nationalism() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    g.add_player(0, 0);
    g.apply_player_upgrade(0, "Upgrade_Nationalism");
    g.apply_player_upgrade(0, "Upgrade_Fanaticism");
    // No horde module at all.
    let id = g.create_object(&unit("Tank", None), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.run_frames(3);
    assert!(!has(&g, id, condition::NATIONALISM), "no HordeUpdate evaluates the bonus");
    assert!(!has(&g, id, condition::FANATICISM));
    let flags = g.weapon_bonus_flags(id);
    assert_eq!(flags & (1 << condition::NATIONALISM), 0, "weapon_bonus_flags must not read the upgrade");
    assert_eq!(flags & (1 << condition::FANATICISM), 0);
}
