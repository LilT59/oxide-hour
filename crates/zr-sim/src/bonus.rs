//! Weapon bonuses (`GameLogic/Object/Weapon.cpp` `WeaponBonus` / `WeaponBonusSet`).
//!
//! A bonus condition bit (from `TheWeaponBonusNames`, `Weapon.h`) applies a multiplier to a field
//! while the condition holds. `appendBonuses` adds each matching entry's excess over 1.0, so the
//! fields start at 1.0 and combine additively.

/// `WeaponBonus::Field`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum WeaponBonusField {
    Damage = 0,
    Radius = 1,
    Range = 2,
    RateOfFire = 3,
    PreAttack = 4,
}

pub const FIELD_COUNT: usize = 5;

impl WeaponBonusField {
    pub fn index(self) -> usize {
        self as usize
    }
}

/// `WeaponBonus`: the five multipliers, all 1.0 by default.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeaponBonus {
    pub fields: [f32; FIELD_COUNT],
}

impl Default for WeaponBonus {
    fn default() -> Self {
        WeaponBonus { fields: [1.0; FIELD_COUNT] }
    }
}

impl WeaponBonus {
    pub fn get(&self, f: WeaponBonusField) -> f32 {
        self.fields[f.index()]
    }
}

/// `WeaponBonusConditionType` bit indices (`Weapon.h`); index == the bit.
pub mod condition {
    pub const GARRISONED: u8 = 0;
    pub const HORDE: u8 = 1;
    pub const CONTINUOUS_FIRE_MEAN: u8 = 2;
    pub const CONTINUOUS_FIRE_FAST: u8 = 3;
    pub const NATIONALISM: u8 = 4;
    pub const PLAYER_UPGRADE: u8 = 5;
    pub const DRONE_SPOTTING: u8 = 6;
    pub const DEMORALIZED_OBSOLETE: u8 = 7;
    pub const ENTHUSIASTIC: u8 = 8;
    pub const VETERAN: u8 = 9;
    pub const ELITE: u8 = 10;
    pub const HERO: u8 = 11;
    pub const BATTLEPLAN_BOMBARDMENT: u8 = 12;
    pub const BATTLEPLAN_HOLDTHELINE: u8 = 13;
    pub const BATTLEPLAN_SEARCHANDDESTROY: u8 = 14;
    pub const SUBLIMINAL: u8 = 15;
    pub const SOLO_HUMAN_EASY: u8 = 16;
    pub const SOLO_HUMAN_NORMAL: u8 = 17;
    pub const SOLO_HUMAN_HARD: u8 = 18;
    pub const SOLO_AI_EASY: u8 = 19;
    pub const SOLO_AI_NORMAL: u8 = 20;
    pub const SOLO_AI_HARD: u8 = 21;
    pub const TARGET_FAERIE_FIRE: u8 = 22;
    pub const FANATICISM: u8 = 23;
    pub const FRENZY_ONE: u8 = 24;
    pub const FRENZY_TWO: u8 = 25;
    pub const FRENZY_THREE: u8 = 26;
}

/// `WeaponBonusSet`: entries are `(condition bit, field index, multiplier)`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WeaponBonusSet {
    pub entries: Vec<(u8, u8, f32)>,
}

impl WeaponBonusSet {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `WeaponBonusSet::appendBonuses`: add `value - 1.0` for each condition in `flags`.
    pub fn append(&self, flags: u32, out: &mut WeaponBonus) {
        if flags == 0 {
            return;
        }
        for &(cond, field, value) in &self.entries {
            if flags & (1u32 << cond) != 0 {
                out.fields[field as usize] += value - 1.0;
            }
        }
    }

    /// The combined bonus for the given condition flags.
    pub fn compute(&self, flags: u32) -> WeaponBonus {
        let mut b = WeaponBonus::default();
        self.append(flags, &mut b);
        b
    }
}
