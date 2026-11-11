//! `ArmorTemplate`. Ported from `GameLogic/Object/Armor.cpp` (`ArmorTemplate::parseArmorCoefficients`,
//! `ArmorStore::parseArmorDefinition`).

use zr_ini::{Block, Item};

use crate::diag::{Diag, WarningKind};
use crate::flags::NameEnum;
use crate::names::DamageType;
use crate::value::Args;

/// Damage-type to damage multiplier. Every coefficient starts at 1.0 (`ArmorTemplate::clear`).
#[derive(Debug, Clone, PartialEq)]
pub struct ArmorTemplate {
    pub name: String,
    /// Indexed by `DamageType as usize`; the fraction is the INI percent / 100.
    pub coefficients: [f32; DamageType::ALL.len()],
}

impl ArmorTemplate {
    pub fn new(name: &str) -> Self {
        ArmorTemplate { name: name.to_string(), coefficients: [1.0; DamageType::ALL.len()] }
    }

    /// The multiplier for one damage type.
    pub fn coefficient(&self, t: DamageType) -> f32 {
        self.coefficients[t.index()]
    }

    /// `ArmorTemplate::adjustDamage`: unresistable damage bypasses armor, the result is clamped at 0.
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

    /// Parses an `Armor NAME ... End` block. `ArmorStore::parseArmorDefinition` calls `clear()` first,
    /// so a repeated definition replaces the earlier one entirely (it does not overlay).
    pub(crate) fn from_block(b: &Block, d: &mut Diag) -> Self {
        let mut a = ArmorTemplate::new(b.name().unwrap_or(""));
        d.push_scope(format!("Armor {}", a.name));
        for item in &b.items {
            let Item::Field(f) = item else {
                d.warn(WarningKind::Structure, "", 0, "unexpected nested block");
                continue;
            };
            if f.key != "Armor" {
                d.unknown_field(f);
                continue;
            }
            let mut args = Args::new(&f.value);
            let r = (|| -> Result<(), String> {
                let name = args.next()?;
                let pct = args.percent()?;
                if name.eq_ignore_ascii_case("Default") {
                    a.coefficients = [pct; DamageType::ALL.len()];
                } else {
                    let dt = DamageType::from_name(name).ok_or_else(|| format!("unknown damage type `{name}`"))?;
                    a.coefficients[dt.index()] = pct;
                }
                Ok(())
            })();
            if let Err(e) = r {
                d.bad_value(f, e);
            }
        }
        d.pop_scope();
        a
    }
}
