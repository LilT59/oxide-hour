//! `ArmorTemplate` (GameLogic/Object/Armor.cpp): per-damage-type multipliers.

use crate::damage::DamageType;

// `ArmorSetType` condition bits (`ArmorSet.h:41-53`). Each bit is `1 << index`, the index of the
// `zr_templates::ArmorSetType`, so they agree with the template's `Mask<ArmorSetType>`
// (`zr-game/tests/set_flag_bits.rs` checks this). The setters touch only their own bit
// (`ActiveBody::setArmorSetFlag`, `ActiveBody.h:97`).
pub const ARMORSET_VETERAN: u32 = 1 << 0;
pub const ARMORSET_ELITE: u32 = 1 << 1;
pub const ARMORSET_HERO: u32 = 1 << 2;
/// Set by `ArmorUpgrade::upgradeImplementation` (`ArmorUpgrade.cpp:98`).
pub const ARMORSET_PLAYER_UPGRADE: u32 = 1 << 3;
pub const ARMORSET_WEAK_VERSUS_BASEDEFENSES: u32 = 1 << 4;
/// `ARMORSET_SECOND_LIFE`: set by `UndeadBody` (`UndeadBody.cpp:117`).
pub const ARMORSET_SECOND_LIFE: u32 = 1 << 5;
/// Salvage crate: `SalvageCrateCollide::doArmorSet` (`SalvageCrateCollide.cpp:212`).
pub const ARMORSET_CRATE_UPGRADE_ONE: u32 = 1 << 6;
pub const ARMORSET_CRATE_UPGRADE_TWO: u32 = 1 << 7;

/// Every coefficient starts at 1.0 (`ArmorTemplate::clear`).
#[derive(Clone, Debug, PartialEq)]
pub struct ArmorTemplate {
    pub name: String,
    pub coefficients: [f32; DamageType::COUNT],
}

impl ArmorTemplate {
    pub fn new(name: &str) -> Self {
        Self { name: name.to_owned(), coefficients: [1.0; DamageType::COUNT] }
    }

    /// `ArmorTemplate::adjustDamage`: unresistable damage bypasses armor; the result is clamped at 0.
    pub fn adjust_damage(&self, t: DamageType, damage: f32) -> f32 {
        if t == DamageType::Unresistable || t == DamageType::SubdualUnresistable {
            return damage;
        }
        let d = damage * self.coefficients[t.index()];
        if d < 0.0 {
            0.0
        } else {
            d
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coefficients_scale_and_unresistable_bypasses() {
        let mut a = ArmorTemplate::new("T");
        a.coefficients[DamageType::SmallArms.index()] = 0.25;
        a.coefficients[DamageType::Unresistable.index()] = 0.0;
        assert_eq!(a.adjust_damage(DamageType::SmallArms, 100.0), 25.0);
        assert_eq!(a.adjust_damage(DamageType::Explosion, 100.0), 100.0);
        assert_eq!(a.adjust_damage(DamageType::Unresistable, 100.0), 100.0);
    }
}
