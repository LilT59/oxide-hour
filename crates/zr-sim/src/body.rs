//! Body modules (Object/Body/ActiveBody.cpp): health and damage.
//!
//! Ported: `attemptDamage` / `attemptHealing` with armor (`m_curArmor.adjustDamage`), health
//! clamping and the dead check; `DAMAGE_HEALING` routed to healing; subdual damage accumulation
//! (`ActiveBody::attemptDamage`'s subdual branch, `ActiveBody.cpp:495-513`, `internalAddSubdualDamage`,
//! `canBeSubdued`, `isSubdued`, `hasAnySubdualDamage`), in the retail CRC form (`RETAIL_COMPATIBLE_CRC`,
//! `ActiveBody.cpp:1264-1266`: capped above, not floored at zero).
//! Not here: the game-side half of a subdual hit (`onSubdualChange`, `notifySubdualDamage`), which
//! `object_helpers.rs` runs from the damage path, and `DAMAGE_STATUS`: a status hit never reaches the body, because
//! the weapon path hands its duration straight to the `StatusDamageHelper` (`combat.rs` `apply_status_damage`).
//! Stubbed: the damage scalar (`m_damageScalar`, strategy-center battle plans), the fear-sound and damage-FX hooks
//! (client), and `DAMAGE_KILL_GARRISONED` / `DAMAGE_DISARM` / `DAMAGE_DEPLOY` (the body drops them and nothing else
//! acts on them yet).

use crate::armor::ArmorTemplate;
use crate::condition::{calc_damage_state, BodyDamageType};
use crate::crc::Crc;
use crate::object::ObjectId;
use std::sync::Arc;

pub use crate::damage::DamageType;

/// `DamageInfo` (input half).
#[derive(Clone, Copy, Debug)]
pub struct DamageInfo {
    pub source: ObjectId,
    pub amount: f32,
    pub kind: DamageType,
}

/// `MaxHealthChangeType` (`BodyModule.h`): how a `MaxHealthUpgrade` affects current health.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MaxHealthChangeType {
    #[default]
    SameCurrentHealth,
    PreserveRatio,
    AddCurrentHealthToo,
}

/// `BodyModuleInterface`.
pub trait BodyModule {
    fn health(&self) -> f32;
    fn max_health(&self) -> f32;
    /// `BodyModuleInterface::getDamageState`: the stored `m_curDamageState`. It is recomputed only where the
    /// engine calls `setCorrectDamageState` (health changes), not on every query.
    fn damage_state(&self) -> BodyDamageType;
    /// Applies damage and returns the amount actually dealt.
    fn attempt_damage(&mut self, info: &DamageInfo) -> f32;
    /// Applies healing and returns the amount actually healed.
    fn attempt_healing(&mut self, amount: f32) -> f32;
    /// `setArmorSetFlag`: switch the armor currently in force.
    fn set_armor(&mut self, _armor: Option<Arc<ArmorTemplate>>) {}
    /// `MaxHealthUpgrade`.
    fn change_max_health(&mut self, add: f32, change_type: MaxHealthChangeType) {
        let _ = (add, change_type);
    }
    /// `BodyModuleInterface::setInitialHealth` (`BodyModule.h:178`, `ActiveBody.cpp:883-895`): set the health to
    /// `initial_percent` of the *initial* health. `InactiveBody` overrides it with an empty body (`BodyModule.h:238`),
    /// which the default here reproduces.
    fn set_initial_health(&mut self, initial_percent: i32) {
        let _ = initial_percent;
    }
    fn is_dead(&self) -> bool {
        self.health() <= 0.0
    }
    /// `getFrontCrushed` / `getBackCrushed` / `setFrontCrushed` / `setBackCrushed`
    /// (`BodyModule.h`): which crush point a `CrushDie` hit (drives the crushed model condition).
    fn front_crushed(&self) -> bool {
        false
    }
    fn back_crushed(&self) -> bool {
        false
    }
    fn set_front_crushed(&mut self, _crushed: bool) {}
    fn set_back_crushed(&mut self, _crushed: bool) {}
    /// `ActiveBody::canBeSubdued`: the body has a `SubdualDamageCap`.
    fn can_be_subdued(&self) -> bool {
        false
    }
    /// `ActiveBody::isSubdued` (retail form, `ActiveBody.cpp:1327-1330`): the subdual damage has reached the max health.
    fn is_subdued(&self) -> bool {
        false
    }
    /// `ActiveBody::hasAnySubdualDamage` (`ActiveBody.cpp:1378`).
    fn has_any_subdual_damage(&self) -> bool {
        false
    }
    /// `m_curArmor.adjustDamage(type, amount)` (ActiveBody.cpp:362), the amount a hit of `kind` deals after armour, for
    /// the hits that do not go through `attempt_damage` (the `DAMAGE_STATUS` duration, which the engine computes after
    /// the same armour step).
    fn adjusted_damage(&self, _kind: DamageType, amount: f32) -> f32 {
        amount
    }
    /// The armour-adjusted amount of the last `attempt_damage` call's subdual hit (the `amount` that
    /// `ActiveBody::attemptDamage` passes to `Object::notifySubdualDamage`), or 0 for any other call.
    fn last_subdual_hit(&self) -> f32 {
        0.0
    }
    /// `getSubdualDamageHealRate` (frames) and `getSubdualDamageHealAmount`.
    fn subdual_heal_rate(&self) -> u32 {
        0
    }
    fn subdual_heal_amount(&self) -> f32 {
        0.0
    }
    fn crc(&self, c: &mut Crc);
}

/// `ActiveBody`.
#[derive(Clone, Debug)]
pub struct ActiveBody {
    health: f32,
    max_health: f32,
    /// `m_initialHealth` (`ActiveBody.h:52`): the template's `InitialHealth`, the base `setInitialHealth`
    /// scales (`ActiveBody.cpp:883-895`). Not the same as `max_health` once a `MaxHealthUpgrade` ran (`:901-905`).
    initial_health: f32,
    armor: Option<Arc<ArmorTemplate>>,
    /// `m_frontCrushed` / `m_backCrushed` (`ActiveBody`).
    front_crushed: bool,
    back_crushed: bool,
    /// `m_curDamageState` (`ActiveBody.h`): stored, and recomputed only by `setCorrectDamageState`
    /// (`ActiveBody.cpp:203`), which `internalChangeHealth` calls (`:1237`) and the constructor (`:181`).
    damage_state: BodyDamageType,
    /// `TheGlobalData->m_unitDamagedThresh` and `m_unitReallyDamagedThresh`, read by `calcDamageState`
    /// (`ActiveBody.cpp:100-124`). The engine reads them live; the body takes them when it is created
    /// (`GameRules` is set before any object is made).
    damaged_threshold: f32,
    really_damaged_threshold: f32,
    /// `ActiveBody::m_currentSubdualDamage` (`ActiveBody.h`).
    subdual_damage: f32,
    /// `ActiveBodyModuleData::m_subdualDamageCap`: 0 means the body cannot be subdued (`canBeSubdued`).
    subdual_cap: f32,
    /// `m_subdualDamageHealRate` (frames) and `m_subdualDamageHealAmount`, read by `SubdualDamageHelper`.
    subdual_heal_rate: u32,
    subdual_heal_amount: f32,
    /// The armour-adjusted subdual amount of the last `attempt_damage` call (see [`BodyModule::last_subdual_hit`]).
    last_subdual_hit: f32,
}

impl ActiveBody {
    pub fn new(max_health: f32, initial_health: f32, damaged_threshold: f32, really_damaged_threshold: f32) -> Self {
        let max_health = max_health.max(0.0);
        let mut b = Self {
            health: initial_health.clamp(0.0, max_health),
            max_health,
            // `m_initialHealth = getActiveBodyModuleData()->m_initialHealth` (`ActiveBody.cpp:176`).
            initial_health,
            armor: None,
            front_crushed: false,
            back_crushed: false,
            damage_state: BodyDamageType::Pristine,
            damaged_threshold,
            really_damaged_threshold,
            subdual_damage: 0.0,
            subdual_cap: 0.0,
            subdual_heal_rate: 0,
            subdual_heal_amount: 0.0,
            last_subdual_hit: 0.0,
        };
        // `ActiveBody` constructor: `setCorrectDamageState()` (`ActiveBody.cpp:181`).
        b.set_correct_damage_state();
        b
    }

    /// `ActiveBody::setCorrectDamageState` (`ActiveBody.cpp:203-206`): the stored state from the health.
    fn set_correct_damage_state(&mut self) {
        self.damage_state = calc_damage_state(self.health, self.max_health, self.damaged_threshold, self.really_damaged_threshold);
    }

    /// `ActiveBody::internalChangeHealth` (`ActiveBody.cpp:1217-1237`): move the health by `delta`, cap it to
    /// `[0, max]`, and recompute the stored damage state. Every health change goes through here, including
    /// a zero change, so a state left stale by `SAME_CURRENTHEALTH` is refreshed by the next change.
    fn internal_change_health(&mut self, delta: f32) {
        self.health += delta;
        if self.health > self.max_health {
            self.health = self.max_health;
        }
        if self.health < 0.0 {
            self.health = 0.0;
        }
        self.set_correct_damage_state();
    }
    /// `ActiveBody::setArmorSetFlag` result: the armor currently in force.
    pub fn with_armor(mut self, armor: Option<Arc<ArmorTemplate>>) -> Self {
        self.armor = armor;
        self
    }
    /// The `ActiveBodyModuleData` subdual fields: `SubdualDamageCap`, `SubdualDamageHealRate` (frames) and
    /// `SubdualDamageHealAmount`. A cap of 0 leaves the body unsubduable (`canBeSubdued`, `ActiveBody.cpp:1277`).
    pub fn with_subdual(mut self, cap: f32, heal_rate: u32, heal_amount: f32) -> Self {
        self.subdual_cap = cap.max(0.0);
        self.subdual_heal_rate = heal_rate;
        self.subdual_heal_amount = heal_amount;
        self
    }
    /// `ActiveBody::getCurrentSubdualDamage`-style read: the accumulated subdual damage (`m_currentSubdualDamage`).
    pub fn subdual_damage(&self) -> f32 {
        self.subdual_damage
    }
    /// `ActiveBody::internalAddSubdualDamage` (`ActiveBody.cpp:1261-1268`, the retail CRC branch): add `delta`
    /// and cap the total at `SubdualDamageCap`. Nothing floors it at zero in retail, so a heal can overshoot.
    fn internal_add_subdual_damage(&mut self, delta: f32) {
        self.subdual_damage = (self.subdual_damage + delta).min(self.subdual_cap);
    }
}

impl BodyModule for ActiveBody {
    fn health(&self) -> f32 {
        self.health
    }
    fn max_health(&self) -> f32 {
        self.max_health
    }
    /// `ActiveBody::setInitialHealth` (`ActiveBody.cpp:883-895`): "simple setting of the health value, it does *NOT*
    /// track any transition states for the event of damage or death". The `initialPercent/100.0f` product is a `Real`
    /// and `internalChangeHealth` takes the delta.
    fn set_initial_health(&mut self, initial_percent: i32) {
        let factor = initial_percent as f32 / 100.0;
        let new_health = factor * self.initial_health;
        self.internal_change_health(new_health - self.health);
    }
    fn damage_state(&self) -> BodyDamageType {
        self.damage_state
    }
    fn attempt_damage(&mut self, info: &DamageInfo) -> f32 {
        self.last_subdual_hit = 0.0;
        if self.health <= 0.0 {
            return 0.0; // already dead
        }
        match info.kind {
            DamageType::Healing => return self.attempt_healing(info.amount),
            // `DAMAGE_STATUS`, `DAMAGE_KILLPILOT` and `DAMAGE_KILL_GARRISONED` are handled by the damage pipeline
            // (`GameLogic::apply_status_damage` / `damage_object_with_death`, which own the contain and the status
            // helpers); the body itself must not touch health for them (`alreadyHandled` in `ActiveBody::attemptDamage`).
            DamageType::Status | DamageType::KillPilot | DamageType::KillGarrisoned | DamageType::Deploy | DamageType::Disarm => return 0.0,
            _ => {}
        }
        // `m_curArmor.adjustDamage` comes before the subdual branch (`ActiveBody.cpp:362`), so subdual is armored too.
        let amount = match &self.armor {
            Some(a) => a.adjust_damage(info.kind, info.amount),
            None => info.amount,
        };
        // The subdual branch (`ActiveBody.cpp:495-513`): a body without a `SubdualDamageCap` ignores it; otherwise
        // the damage accumulates and health is untouched. A heal is a negative subdual hit (`SubdualDamageHelper::update`).
        if info.kind.is_subdual() {
            if self.can_be_subdued() {
                self.internal_add_subdual_damage(amount);
                self.last_subdual_hit = amount;
            }
            return 0.0;
        }
        if amount <= 0.0 {
            return 0.0;
        }
        // `ActiveBody::attemptDamage` (`:534-540`): `internalChangeHealth(-amount)` (`:540`).
        let dealt = if amount > self.health { self.health } else { amount };
        self.internal_change_health(-dealt);
        dealt
    }
    fn attempt_healing(&mut self, amount: f32) -> f32 {
        // Healing works on a rubble body too (a dozer repairs a destroyed bridge); `is_dead` stays
        // true until health is positive again.
        if amount <= 0.0 {
            return 0.0;
        }
        // `ActiveBody::attemptHealing` (`ActiveBody.cpp:835`): `internalChangeHealth(amount)`, capped there.
        let old = self.health;
        self.internal_change_health(amount);
        self.health - old
    }
    fn set_armor(&mut self, armor: Option<Arc<ArmorTemplate>>) {
        self.armor = armor;
    }
    /// `ActiveBody::setMaxHealth` (`ActiveBody.cpp:901-950`). Only `SAME_CURRENTHEALTH` leaves the stored
    /// damage state alone; the other types change health through `internalChangeHealth`, which recomputes it.
    fn change_max_health(&mut self, add: f32, change_type: MaxHealthChangeType) {
        let prev_max = self.max_health;
        self.max_health = (self.max_health + add).max(0.0);
        // `m_initialHealth = maxHealth` (`ActiveBody.cpp:905`).
        self.initial_health = self.max_health;
        match change_type {
            // `do nothing` (`:927`).
            MaxHealthChangeType::SameCurrentHealth => {}
            // `:920-925`: the same amount that is added to the max is added to the health.
            MaxHealthChangeType::AddCurrentHealthToo => self.internal_change_health(self.max_health - prev_max),
            // `:908-916`: keep the health ratio; `internalChangeHealth` runs even for a zero change.
            MaxHealthChangeType::PreserveRatio => {
                let new_health = if prev_max > 0.0 { self.health / prev_max * self.max_health } else { self.health };
                self.internal_change_health(new_health - self.health);
            }
        }
        // `:946-950`: a max that falls below the current health clips the health (through `internalChangeHealth`).
        if self.health > self.max_health {
            self.internal_change_health(self.max_health - self.health);
        }
    }
    fn front_crushed(&self) -> bool {
        self.front_crushed
    }
    fn back_crushed(&self) -> bool {
        self.back_crushed
    }
    fn set_front_crushed(&mut self, crushed: bool) {
        self.front_crushed = crushed;
    }
    fn set_back_crushed(&mut self, crushed: bool) {
        self.back_crushed = crushed;
    }
    fn can_be_subdued(&self) -> bool {
        self.subdual_cap > 0.0
    }
    fn is_subdued(&self) -> bool {
        // `ActiveBody::isSubdued`, retail CRC form (`ActiveBody.cpp:1327-1330`).
        self.max_health <= self.subdual_damage
    }
    fn has_any_subdual_damage(&self) -> bool {
        self.subdual_damage > 0.0
    }
    fn subdual_heal_rate(&self) -> u32 {
        self.subdual_heal_rate
    }
    fn subdual_heal_amount(&self) -> f32 {
        self.subdual_heal_amount
    }
    fn last_subdual_hit(&self) -> f32 {
        self.last_subdual_hit
    }
    fn adjusted_damage(&self, kind: DamageType, amount: f32) -> f32 {
        match &self.armor {
            Some(a) => a.adjust_damage(kind, amount),
            None => amount,
        }
    }
    fn crc(&self, c: &mut Crc) {
        c.add_f32(self.health);
        c.add_f32(self.max_health);
        // Only hashed when set, so objects that are never crushed keep their historical CRC.
        if self.front_crushed || self.back_crushed {
            c.add_bool(self.front_crushed);
            c.add_bool(self.back_crushed);
        }
        // Likewise for subdual damage: an unsubdued body hashes as it did before.
        if self.subdual_damage != 0.0 {
            c.add_f32(self.subdual_damage);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `UnitDamagedThreshold` / `UnitReallyDamagedThreshold` used by these tests.
    const DAMAGED: f32 = 0.7;
    const REALLY: f32 = 0.35;

    fn small_arms(amount: f32) -> DamageInfo {
        DamageInfo { source: ObjectId::INVALID, amount, kind: DamageType::SmallArms }
    }

    #[test]
    fn damage_and_heal_clamp() {
        let mut b = ActiveBody::new(100.0, 100.0, DAMAGED, REALLY);
        let d = DamageInfo { source: ObjectId::INVALID, amount: 30.0, kind: DamageType::SmallArms };
        assert_eq!(b.attempt_damage(&d), 30.0);
        assert_eq!(b.attempt_healing(50.0), 30.0);
        assert_eq!(b.health(), 100.0);
        let big = DamageInfo { amount: 1000.0, ..d };
        assert_eq!(b.attempt_damage(&big), 100.0);
        assert!(b.is_dead());
        // A rubble body can be healed back (dozer bridge repair).
        assert_eq!(b.attempt_healing(10.0), 10.0);
        assert!(!b.is_dead());
    }

    /// `ActiveBody` constructor: `setCorrectDamageState()` (`ActiveBody.cpp:181`). Reverting the call in `new`
    /// leaves a damaged start at Pristine, so this fails.
    #[test]
    fn the_state_is_set_at_construction() {
        assert_eq!(ActiveBody::new(100.0, 60.0, DAMAGED, REALLY).damage_state(), BodyDamageType::Damaged);
        assert_eq!(ActiveBody::new(100.0, 20.0, DAMAGED, REALLY).damage_state(), BodyDamageType::ReallyDamaged);
    }

    /// Damage and healing recompute the state (`attemptDamage` / `attemptHealing` through `internalChangeHealth`).
    #[test]
    fn damage_and_healing_move_the_stored_state() {
        let mut b = ActiveBody::new(100.0, 100.0, DAMAGED, REALLY);
        assert_eq!(b.attempt_damage(&small_arms(40.0)), 40.0);
        assert_eq!(b.damage_state(), BodyDamageType::Damaged);
        assert_eq!(b.attempt_damage(&small_arms(30.0)), 30.0);
        assert_eq!(b.damage_state(), BodyDamageType::ReallyDamaged);
        assert_eq!(b.attempt_healing(100.0), 70.0);
        assert_eq!(b.damage_state(), BodyDamageType::Pristine);
    }

    /// `SAME_CURRENTHEALTH` changes the max and nothing else (`ActiveBody.cpp:927`), so a damaged body keeps its
    /// stored state (and its locomotor penalty) until the next damage or heal. Recomputing on the max change
    /// (the reverted fix) makes the first assert fail: 60 of 200 is a ratio of 0.3, really damaged.
    #[test]
    fn a_same_current_health_max_change_keeps_the_stored_state() {
        let mut b = ActiveBody::new(100.0, 100.0, DAMAGED, REALLY);
        b.attempt_damage(&small_arms(40.0));
        assert_eq!(b.damage_state(), BodyDamageType::Damaged);
        b.change_max_health(100.0, MaxHealthChangeType::SameCurrentHealth);
        assert_eq!((b.health(), b.max_health()), (60.0, 200.0));
        assert_eq!(b.damage_state(), BodyDamageType::Damaged, "the stored state is not recomputed");
        // The next heal recomputes it from the health: 61 of 200 is really damaged.
        b.attempt_healing(1.0);
        assert_eq!(b.damage_state(), BodyDamageType::ReallyDamaged);
    }

    /// A hit that deals nothing returns before `internalChangeHealth` (`attemptDamage`), so it leaves a stale
    /// state as it is.
    #[test]
    fn a_zero_hit_keeps_a_stale_state() {
        let mut b = ActiveBody::new(100.0, 100.0, DAMAGED, REALLY);
        b.attempt_damage(&small_arms(40.0));
        b.change_max_health(100.0, MaxHealthChangeType::SameCurrentHealth);
        assert_eq!(b.attempt_damage(&small_arms(0.0)), 0.0);
        assert_eq!(b.damage_state(), BodyDamageType::Damaged);
    }

    /// `PRESERVE_RATIO` runs `internalChangeHealth` even for a zero change (`ActiveBody.cpp:915`), so the
    /// stale state is refreshed: 60 of 200 keeps the 0.3 ratio, really damaged.
    #[test]
    fn a_preserve_ratio_max_change_recomputes_the_state() {
        let mut b = ActiveBody::new(100.0, 100.0, DAMAGED, REALLY);
        b.attempt_damage(&small_arms(40.0));
        b.change_max_health(100.0, MaxHealthChangeType::SameCurrentHealth);
        assert_eq!(b.damage_state(), BodyDamageType::Damaged);
        b.change_max_health(0.0, MaxHealthChangeType::PreserveRatio);
        assert!((b.health() - 60.0).abs() < 1e-3, "the ratio is kept");
        assert_eq!(b.damage_state(), BodyDamageType::ReallyDamaged);
    }

    /// `ADD_CURRENT_HEALTH_TOO` also goes through `internalChangeHealth` (`ActiveBody.cpp:924`).
    #[test]
    fn an_add_current_health_max_change_recomputes_the_state() {
        let mut b = ActiveBody::new(100.0, 100.0, DAMAGED, REALLY);
        b.attempt_damage(&small_arms(40.0));
        b.change_max_health(100.0, MaxHealthChangeType::SameCurrentHealth);
        // +100 max and +100 health: 160 of 300 is 0.53, damaged.
        b.change_max_health(100.0, MaxHealthChangeType::AddCurrentHealthToo);
        assert_eq!(b.health(), 160.0);
        assert_eq!(b.damage_state(), BodyDamageType::Damaged);
    }

    /// A max below the current health clips it through `internalChangeHealth` (`ActiveBody.cpp:946-950`), which
    /// recomputes the state: 50 of 50 is pristine. Without the clip the stored state would stay damaged.
    #[test]
    fn clipping_the_health_recomputes_the_state() {
        let mut b = ActiveBody::new(100.0, 100.0, DAMAGED, REALLY);
        b.attempt_damage(&small_arms(40.0));
        b.change_max_health(-50.0, MaxHealthChangeType::SameCurrentHealth);
        assert_eq!((b.health(), b.max_health()), (50.0, 50.0));
        assert_eq!(b.damage_state(), BodyDamageType::Pristine);
    }

    fn subdual(amount: f32) -> DamageInfo {
        DamageInfo { source: ObjectId::INVALID, amount, kind: DamageType::SubdualVehicle }
    }

    /// `ActiveBody::attemptDamage`'s subdual branch (`ActiveBody.cpp:495-513`): the damage accumulates and health
    /// is untouched. Before the subdual branch existed the body returned 0 for every subdual hit, so the first
    /// assert of the damage total fails with that code.
    #[test]
    fn subdual_damage_accumulates_and_leaves_health_alone() {
        let mut b = ActiveBody::new(100.0, 100.0, DAMAGED, REALLY).with_subdual(1000.0, 15, 50.0);
        assert_eq!(b.attempt_damage(&subdual(30.0)), 0.0);
        assert_eq!(b.attempt_damage(&subdual(20.0)), 0.0);
        assert_eq!(b.health(), 100.0);
        assert!(b.has_any_subdual_damage());
        assert!(!b.is_subdued(), "50 of 100 max health is not subdued");
        assert_eq!(b.damage_state(), BodyDamageType::Pristine);
    }

    /// `internalAddSubdualDamage` caps the total at `SubdualDamageCap`, and `isSubdued` is `max <= subdual`
    /// (`ActiveBody.cpp:1264-1266`, `:1327-1330`, the retail CRC branch).
    #[test]
    fn subdual_damage_is_capped_and_subdues_at_max_health() {
        let mut b = ActiveBody::new(100.0, 100.0, DAMAGED, REALLY).with_subdual(150.0, 15, 50.0);
        b.attempt_damage(&subdual(120.0));
        b.attempt_damage(&subdual(120.0));
        assert_eq!(b.subdual_damage(), 150.0);
        assert!(b.is_subdued());
    }

    /// `canBeSubdued` is `SubdualDamageCap > 0` (`ActiveBody.cpp:1277`): without one the subdual hit is ignored.
    #[test]
    fn a_body_without_a_subdual_cap_ignores_subdual_damage() {
        let mut b = ActiveBody::new(100.0, 100.0, DAMAGED, REALLY);
        assert!(!b.can_be_subdued());
        b.attempt_damage(&subdual(50.0));
        assert!(!b.has_any_subdual_damage());
        assert!(!b.is_subdued());
        assert_eq!(b.health(), 100.0);
    }

    /// A heal is a `SUBDUAL_UNRESISTABLE` hit of minus the heal amount (`SubdualDamageHelper::update`). Retail
    /// does not floor the total at zero, so 30 healed by 50 leaves -20 (`ActiveBody.cpp:1264-1266`), which
    /// `has_any_subdual_damage` reads as none.
    #[test]
    fn a_subdual_heal_is_a_negative_hit_and_is_not_floored() {
        let mut b = ActiveBody::new(100.0, 100.0, DAMAGED, REALLY).with_subdual(1000.0, 15, 50.0);
        b.attempt_damage(&subdual(30.0));
        let heal = DamageInfo { source: ObjectId::INVALID, amount: -b.subdual_heal_amount(), kind: DamageType::SubdualUnresistable };
        b.attempt_damage(&heal);
        assert_eq!(b.subdual_damage(), -20.0);
        assert!(!b.has_any_subdual_damage());
        assert_eq!(b.health(), 100.0);
    }
}
