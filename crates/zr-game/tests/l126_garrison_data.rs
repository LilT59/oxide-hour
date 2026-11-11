//! Lane 126: the shipped `GarrisonContain` `ImmuneToClearBuildingAttacks` flag and the
//! `AllowAttackGarrisonedBldgs` weapon field reach the sim through the adapter. Skips when no install is found.

use zr_assets::{Installs, Vfs};
use zr_game::GameData;
use zr_sim::template::TemplateSource;

fn vfs() -> Option<Vfs> {
    let zh = Installs::detect().zero_hour?;
    Vfs::zero_hour(&zh).ok()
}

/// `GarrisonContainModuleData::m_immuneToClearBuildingAttacks` (`GarrisonContain.h:72`, `:113`): the China Bunker,
/// GLA Palace and America FireBase set it, a garrisonable civilian building does not. Reverting the adapter line
/// (always false) fails this test.
#[test]
fn the_shipped_immune_garrisons_carry_the_flag() {
    let Some(vfs) = vfs() else { return };
    let data = GameData::load(&vfs);
    let contain = |name: &str| {
        data.store
            .find_template(name)
            .unwrap_or_else(|| panic!("{name} missing"))
            .contain
            .clone()
            .unwrap_or_else(|| panic!("{name} has no contain module"))
    };
    for name in ["ChinaBunker", "GLAPalace", "AmericaFireBase"] {
        let c = contain(name);
        assert!(c.is_garrisonable(), "{name} is a garrison");
        assert!(c.is_immune_to_clear_building_attacks(), "{name} sets ImmuneToClearBuildingAttacks");
    }
    // A garrisonable structure without the flag: the civilian town hall has a GarrisonContain and no immunity.
    let hall = contain("AsiaTownHall01");
    assert!(hall.is_garrisonable(), "the town hall is garrisonable");
    assert!(!hall.is_immune_to_clear_building_attacks(), "and clearable");
}

/// `WeaponTemplate::m_allowAttackGarrisonedBldgs` (`AllowAttackGarrisonedBldgs`, `Weapon.cpp:235`): the Ranger's
/// flash-bang weapon sets it, and its `DAMAGE_SURRENDER` type is what `WeaponSet::getAbleToUseWeaponAgainstTarget`
/// uses to target a garrisoned building (`Weapon.cpp:632-641`); the Microwave Tank's clear-building weapon does not.
#[test]
fn the_shipped_garrison_weapons_parse_the_flag() {
    let Some(vfs) = vfs() else { return };
    let data = GameData::load(&vfs);
    let slot = |template: &str, set_flag: u32, i: usize| {
        let t = data.store.find_template(template).unwrap_or_else(|| panic!("{template} missing"));
        let set = t
            .weapon_sets
            .iter()
            .find(|(cond, _)| *cond == set_flag)
            .map(|(_, s)| s)
            .unwrap_or_else(|| panic!("{template} has no weapon set 0x{set_flag:x}"));
        set.weapons[i].clone().unwrap_or_else(|| panic!("{template} slot {i} missing"))
    };
    let flash = slot("AmericaInfantryRanger", zr_sim::weapon::WEAPONSET_PLAYER_UPGRADE, 1);
    assert!(flash.allow_attack_garrisoned_bldgs, "the flash-bang sets AllowAttackGarrisonedBldgs");
    assert_eq!(flash.damage_type, zr_sim::damage::DamageType::Surrender);
    let clear = slot("AmericaTankMicrowave", 0, 1);
    assert_eq!(clear.damage_type, zr_sim::damage::DamageType::KillGarrisoned);
    assert!(!clear.allow_attack_garrisoned_bldgs, "the microwave's clear weapon does not need the flag");
}
