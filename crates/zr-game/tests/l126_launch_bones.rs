//! Lane 126: `W3DModelDraw::findBestInfo` for the projectile launch matrix. The resolver used to read only the
//! `DefaultConditionState`, so objects whose first state is `ConditionState = NONE` (22 of the 202 launch-bone objects)
//! got no launch bones at all. `zr_game::launch_bones` now resolves the state `findBestInfo` picks for the empty
//! condition set and keeps every state's bones for a fire-time pick. Skips when no install is found.

use zr_assets::{Installs, Vfs};
use zr_game::GameData;
use zr_sim::condition::{bit_for_name, ModelConditionFlags};
use zr_sim::template::TemplateSource;

fn vfs() -> Option<Vfs> {
    let zh = Installs::detect().zero_hour?;
    Vfs::zero_hour(&zh).ok()
}

fn launch_bones(name: &str) -> Option<zr_sim::weapon::LaunchBones> {
    let vfs = vfs()?;
    let data = GameData::load(&vfs);
    data.store.find_template(name).map(|t| t.combat.launch_bones.clone())
}

/// `AmericaTankPaladin` (and the Overlord / Medic) has no `DefaultConditionState`: its `ConditionState = NONE` state holds
/// the `WeaponLaunchBone`, and `findBestInfo` with the empty condition set picks it. Resolving `default_state()` instead
/// finds nothing and the projectile would launch from the object's origin. Fails without the `find_best_state` fix.
#[test]
fn a_no_default_state_object_gets_its_launch_bones() {
    for name in ["AmericaTankPaladin", "ChinaTankOverlord", "AmericaVehicleMedic"] {
        let Some(lb) = launch_bones(name) else { return };
        assert!(
            lb.slots.iter().any(|s| !s.is_empty()),
            "{name} has a ConditionState = NONE launch bone and must resolve it"
        );
    }
}

/// The states are kept by condition, and `LaunchBones::select` picks them like the engine: `ChinaInfantryAgent`'s
/// `FIRING_B` state carries the only `WeaponLaunchBone = SECONDARY Muzzle`, so the empty condition set has none and the
/// firing-B set has one. Fails if the resolver only keeps the default state.
#[test]
fn the_state_table_selects_a_firing_state() {
    let Some(lb) = launch_bones("ChinaInfantryAgent") else { return };
    let (default, _) = lb.select(0);
    assert!(default.iter().all(|s| s.is_empty()), "the default state has no launch bone");
    let firing_b = 1u128 << bit_for_name("FIRING_B").expect("FIRING_B is a model condition");
    let (slots, _) = lb.select(firing_b);
    assert!(!slots[1].is_empty(), "the FIRING_B state's SECONDARY launch bone is selected");
    assert!(slots[0].is_empty(), "and only the secondary slot");
}

/// A crate upgrade is a model condition (`Object::setWeaponSetFlag` mirrors the weapon-set flag,
/// `TheWeaponSetTypeToModelConditionTypeMap`), so the table can pick a crate-upgrade state's bones.
#[test]
fn the_state_table_matches_a_crate_upgrade_condition() {
    let Some(lb) = launch_bones("AmericaTankPaladin") else { return };
    // The Paladin's three states all carry the same TurretMS barrel; the query with REALLYDAMAGED still picks a state
    // (the really-damaged one) and returns its bone instead of falling back to the origin.
    let mut really = ModelConditionFlags::default();
    really.set(bit_for_name("REALLYDAMAGED").unwrap());
    let (slots, _) = lb.select(really.bits());
    assert!(!slots[0].is_empty(), "REALLYDAMAGED picks the really-damaged state's turret bone");
}
