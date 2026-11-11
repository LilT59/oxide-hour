//! The sim's weapon-set and armor-set condition bits must equal `1 << index` of the template enums
//! (`zr_templates::WeaponSetType` / `ArmorSetType`), because the adapter builds the template masks with
//! that index. A mismatch would select the wrong set (the old `CRATEUPGRADE_*` bits were rider bits).

use zr_sim::armor as sim_armor;
use zr_sim::weapon as sim_weapon;
use zr_templates::{ArmorSetType, NameEnum, WeaponSetType};

#[test]
fn weapon_set_bits_match_the_template_enum() {
    let pairs: [(WeaponSetType, u32); 17] = [
        (WeaponSetType::Veteran, sim_weapon::WEAPONSET_VETERAN),
        (WeaponSetType::Elite, sim_weapon::WEAPONSET_ELITE),
        (WeaponSetType::Hero, sim_weapon::WEAPONSET_HERO),
        (WeaponSetType::PlayerUpgrade, sim_weapon::WEAPONSET_PLAYER_UPGRADE),
        (WeaponSetType::CrateupgradeOne, sim_weapon::WEAPONSET_CRATEUPGRADE_ONE),
        (WeaponSetType::CrateupgradeTwo, sim_weapon::WEAPONSET_CRATEUPGRADE_TWO),
        (WeaponSetType::VehicleHijack, sim_weapon::WEAPONSET_VEHICLE_HIJACK),
        (WeaponSetType::Carbomb, sim_weapon::WEAPONSET_CARBOMB),
        (WeaponSetType::MineClearingDetail, sim_weapon::WEAPONSET_MINE_CLEARING_DETAIL),
        (WeaponSetType::WeaponRider1, sim_weapon::WEAPONSET_RIDER1),
        (WeaponSetType::WeaponRider2, sim_weapon::WEAPONSET_RIDER2),
        (WeaponSetType::WeaponRider3, sim_weapon::WEAPONSET_RIDER3),
        (WeaponSetType::WeaponRider4, sim_weapon::WEAPONSET_RIDER4),
        (WeaponSetType::WeaponRider5, sim_weapon::WEAPONSET_RIDER5),
        (WeaponSetType::WeaponRider6, sim_weapon::WEAPONSET_RIDER6),
        (WeaponSetType::WeaponRider7, sim_weapon::WEAPONSET_RIDER7),
        (WeaponSetType::WeaponRider8, sim_weapon::WEAPONSET_RIDER8),
    ];
    assert_eq!(pairs.len(), WeaponSetType::NAMES.len(), "every WeaponSetType has a sim constant");
    for (ty, bit) in pairs {
        assert_eq!(bit, 1 << ty.index(), "WEAPONSET bit for {}", ty.name());
    }
}

#[test]
fn armor_set_bits_match_the_template_enum() {
    let pairs: [(ArmorSetType, u32); 8] = [
        (ArmorSetType::Veteran, sim_armor::ARMORSET_VETERAN),
        (ArmorSetType::Elite, sim_armor::ARMORSET_ELITE),
        (ArmorSetType::Hero, sim_armor::ARMORSET_HERO),
        (ArmorSetType::PlayerUpgrade, sim_armor::ARMORSET_PLAYER_UPGRADE),
        (ArmorSetType::WeakVersusBasedefenses, sim_armor::ARMORSET_WEAK_VERSUS_BASEDEFENSES),
        (ArmorSetType::SecondLife, sim_armor::ARMORSET_SECOND_LIFE),
        (ArmorSetType::CrateUpgradeOne, sim_armor::ARMORSET_CRATE_UPGRADE_ONE),
        (ArmorSetType::CrateUpgradeTwo, sim_armor::ARMORSET_CRATE_UPGRADE_TWO),
    ];
    assert_eq!(pairs.len(), ArmorSetType::NAMES.len(), "every ArmorSetType has a sim constant");
    for (ty, bit) in pairs {
        assert_eq!(bit, 1 << ty.index(), "ARMORSET bit for {}", ty.name());
    }
}

/// `ObjectStatusMaskType` bits: the sim's `object::status` constants are `1 << index` of the template
/// `ObjectStatus` enum, which follows `ObjectStatusTypes.h`. The adapter converts template masks
/// bit-for-bit, so a mismatch would gate die modules on the wrong status (the old `IS_CARBOMB` and
/// `HIJACKED` bits were `PARACHUTING` and `REPULSOR`). `NONE` has no bit.
#[test]
fn object_status_bits_match_the_template_enum() {
    use zr_sim::object::status as s;
    use zr_templates::ObjectStatus as O;
    let pairs: [(O, u64); 44] = [
        (O::Destroyed, s::DESTROYED),
        (O::CanAttack, s::CAN_ATTACK),
        (O::UnderConstruction, s::UNDER_CONSTRUCTION),
        (O::Unselectable, s::UNSELECTABLE),
        (O::NoCollisions, s::NO_COLLISIONS),
        (O::NoAttack, s::NO_ATTACK),
        (O::AirborneTarget, s::AIRBORNE_TARGET),
        (O::Parachuting, s::PARACHUTING),
        (O::Repulsor, s::REPULSOR),
        (O::Hijacked, s::HIJACKED),
        (O::Aflame, s::AFLAME),
        (O::Burned, s::BURNED),
        (O::Wet, s::WET),
        (O::IsFiringWeapon, s::IS_FIRING_WEAPON),
        (O::IsBraking, s::IS_BRAKING),
        (O::Stealthed, s::STEALTHED),
        (O::Detected, s::DETECTED),
        (O::CanStealth, s::CAN_STEALTH),
        (O::Sold, s::SOLD),
        (O::UndergoingRepair, s::UNDERGOING_REPAIR),
        (O::Reconstructing, s::RECONSTRUCTING),
        (O::Masked, s::MASKED),
        (O::IsAttacking, s::IS_ATTACKING),
        (O::UsingAbility, s::USING_ABILITY),
        (O::IsAimingWeapon, s::IS_AIMING_WEAPON),
        (O::NoAttackFromAi, s::NO_ATTACK_FROM_AI),
        (O::IgnoringStealth, s::IGNORING_STEALTH),
        (O::IsCarbomb, s::IS_CARBOMB),
        (O::DeckHeightOffset, s::DECK_HEIGHT_OFFSET),
        (O::StatusRider1, s::RIDER1),
        (O::StatusRider2, s::RIDER2),
        (O::StatusRider3, s::RIDER3),
        (O::StatusRider4, s::RIDER4),
        (O::StatusRider5, s::RIDER5),
        (O::StatusRider6, s::RIDER6),
        (O::StatusRider7, s::RIDER7),
        (O::StatusRider8, s::RIDER8),
        (O::FaerieFire, s::FAERIE_FIRE),
        (O::KillingSelf, s::KILLING_SELF),
        (O::ReassignParking, s::REASSIGN_PARKING),
        (O::BoobyTrapped, s::BOOBY_TRAPPED),
        (O::Immobile, s::IMMOBILE),
        (O::Disguised, s::DISGUISED),
        (O::Deployed, s::DEPLOYED),
    ];
    assert_eq!(pairs.len() + 1, O::NAMES.len(), "every ObjectStatus except NONE has a sim constant");
    for (ty, bit) in pairs {
        assert_eq!(bit, 1 << ty.index(), "status bit for {}", ty.name());
    }
}
