//! `ModelConditionFlags` (`Common/ModelState.h` / `Common/BitFlags.cpp`), the per-object
//! condition bits the draw modules use to pick a model and animation.
//!
//! The engine stores the flags on the `Drawable` and sets them from `Object` state
//! (`Object::setModelConditionState`, `WeaponSet::getModelConditionForWeaponSlot`,
//! `ActiveBody::calcDamageState`, `AIStates.cpp`). The simulation has no drawable, so
//! [`derive`] recomputes the bits from the simulation state once per frame; the client reads
//! them through [`crate::GameLogic::model_conditions`].

use crate::combat::AttackPhase;
use crate::object::Object;
use crate::weapon::{WeaponStatus, WEAPON_SLOT_COUNT};

/// `ModelConditionFlags` bit names, in `s_bitNameList` order (the `ALLOW_SURRENDER` entry is
/// omitted, as in retail builds). Used by the client to match condition states by name.
pub const NAMES: &[&str] = &[
    "TOPPLED",
    "FRONTCRUSHED",
    "BACKCRUSHED",
    "DAMAGED",
    "REALLYDAMAGED",
    "RUBBLE",
    "SPECIAL_DAMAGED",
    "NIGHT",
    "SNOW",
    "PARACHUTING",
    "GARRISONED",
    "ENEMYNEAR",
    "WEAPONSET_VETERAN",
    "WEAPONSET_ELITE",
    "WEAPONSET_HERO",
    "WEAPONSET_CRATEUPGRADE_ONE",
    "WEAPONSET_CRATEUPGRADE_TWO",
    "WEAPONSET_PLAYER_UPGRADE",
    "DOOR_1_OPENING",
    "DOOR_1_CLOSING",
    "DOOR_1_WAITING_OPEN",
    "DOOR_1_WAITING_TO_CLOSE",
    "DOOR_2_OPENING",
    "DOOR_2_CLOSING",
    "DOOR_2_WAITING_OPEN",
    "DOOR_2_WAITING_TO_CLOSE",
    "DOOR_3_OPENING",
    "DOOR_3_CLOSING",
    "DOOR_3_WAITING_OPEN",
    "DOOR_3_WAITING_TO_CLOSE",
    "DOOR_4_OPENING",
    "DOOR_4_CLOSING",
    "DOOR_4_WAITING_OPEN",
    "DOOR_4_WAITING_TO_CLOSE",
    "ATTACKING",
    "PREATTACK_A",
    "FIRING_A",
    "BETWEEN_FIRING_SHOTS_A",
    "RELOADING_A",
    "PREATTACK_B",
    "FIRING_B",
    "BETWEEN_FIRING_SHOTS_B",
    "RELOADING_B",
    "PREATTACK_C",
    "FIRING_C",
    "BETWEEN_FIRING_SHOTS_C",
    "RELOADING_C",
    "TURRET_ROTATE",
    "POST_COLLAPSE",
    "MOVING",
    "DYING",
    "AWAITING_CONSTRUCTION",
    "PARTIALLY_CONSTRUCTED",
    "ACTIVELY_BEING_CONSTRUCTED",
    "PRONE",
    "FREEFALL",
    "ACTIVELY_CONSTRUCTING",
    "CONSTRUCTION_COMPLETE",
    "RADAR_EXTENDING",
    "RADAR_UPGRADED",
    "PANICKING",
    "AFLAME",
    "SMOLDERING",
    "BURNED",
    "DOCKING",
    "DOCKING_BEGINNING",
    "DOCKING_ACTIVE",
    "DOCKING_ENDING",
    "CARRYING",
    "FLOODED",
    "LOADED",
    "JETAFTERBURNER",
    "JETEXHAUST",
    "PACKING",
    "UNPACKING",
    "DEPLOYED",
    "OVER_WATER",
    "POWER_PLANT_UPGRADED",
    "CLIMBING",
    "SOLD",
    "RAPPELLING",
    "ARMED",
    "POWER_PLANT_UPGRADING",
    "SPECIAL_CHEERING",
    "CONTINUOUS_FIRE_SLOW",
    "CONTINUOUS_FIRE_MEAN",
    "CONTINUOUS_FIRE_FAST",
    "RAISING_FLAG",
    "CAPTURED",
    "EXPLODED_FLAILING",
    "EXPLODED_BOUNCING",
    "SPLATTED",
    "USING_WEAPON_A",
    "USING_WEAPON_B",
    "USING_WEAPON_C",
    "PREORDER",
    "CENTER_TO_LEFT",
    "LEFT_TO_CENTER",
    "CENTER_TO_RIGHT",
    "RIGHT_TO_CENTER",
    "RIDER1",
    "RIDER2",
    "RIDER3",
    "RIDER4",
    "RIDER5",
    "RIDER6",
    "RIDER7",
    "RIDER8",
    "STUNNED_FLAILING",
    "STUNNED",
    "SECOND_LIFE",
    "JAMMED",
    "ARMORSET_CRATEUPGRADE_ONE",
    "ARMORSET_CRATEUPGRADE_TWO",
    "USER_1",
    "USER_2",
    "DISGUISED",
];

pub const DAMAGED: u32 = 3;
pub const REALLYDAMAGED: u32 = 4;
pub const RUBBLE: u32 = 5;
pub const DOOR_1_OPENING: u32 = 18;
pub const DOOR_1_CLOSING: u32 = 19;
pub const DOOR_1_WAITING_OPEN: u32 = 20;
pub const DOOR_1_WAITING_TO_CLOSE: u32 = 21;
pub const JETAFTERBURNER: u32 = 71;
pub const PACKING: u32 = 73;
pub const UNPACKING: u32 = 74;
pub const DEPLOYED: u32 = 75;
pub const ATTACKING: u32 = 34;
pub const RADAR_EXTENDING: u32 = 58;
pub const RADAR_UPGRADED: u32 = 59;
pub const PREATTACK_A: u32 = 35;
pub const FIRING_A: u32 = 36;
pub const BETWEEN_FIRING_SHOTS_A: u32 = 37;
pub const RELOADING_A: u32 = 38;
pub const PREATTACK_B: u32 = 39;
pub const FIRING_B: u32 = 40;
pub const BETWEEN_FIRING_SHOTS_B: u32 = 41;
pub const RELOADING_B: u32 = 42;
pub const PREATTACK_C: u32 = 43;
pub const FIRING_C: u32 = 44;
pub const BETWEEN_FIRING_SHOTS_C: u32 = 45;
pub const RELOADING_C: u32 = 46;
pub const TURRET_ROTATE: u32 = 47;
pub const MOVING: u32 = 49;
pub const DYING: u32 = 50;
pub const AWAITING_CONSTRUCTION: u32 = 51;
pub const PARTIALLY_CONSTRUCTED: u32 = 52;
pub const ACTIVELY_BEING_CONSTRUCTED: u32 = 53;
pub const ACTIVELY_CONSTRUCTING: u32 = 56;
pub const CARRYING: u32 = 68;
pub const USING_WEAPON_A: u32 = 92;
pub const USING_WEAPON_B: u32 = 93;
pub const USING_WEAPON_C: u32 = 94;
pub const CONTINUOUS_FIRE_SLOW: u32 = 84;
pub const CONTINUOUS_FIRE_MEAN: u32 = 85;
pub const CONTINUOUS_FIRE_FAST: u32 = 86;

const FIRING: [u32; WEAPON_SLOT_COUNT] = [FIRING_A, FIRING_B, FIRING_C];
const BETWEEN: [u32; WEAPON_SLOT_COUNT] = [BETWEEN_FIRING_SHOTS_A, BETWEEN_FIRING_SHOTS_B, BETWEEN_FIRING_SHOTS_C];
const RELOADING: [u32; WEAPON_SLOT_COUNT] = [RELOADING_A, RELOADING_B, RELOADING_C];
const PREATTACK: [u32; WEAPON_SLOT_COUNT] = [PREATTACK_A, PREATTACK_B, PREATTACK_C];
const USING: [u32; WEAPON_SLOT_COUNT] = [USING_WEAPON_A, USING_WEAPON_B, USING_WEAPON_C];

/// A set of `ModelConditionFlags` bits (`BitFlags<MODELCONDITION_COUNT>`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct ModelConditionFlags(pub u128);

/// The bit index for a condition name (`ModelConditionFlags::parseSingleBitFromINI`).
pub fn bit_for_name(name: &str) -> Option<u32> {
    NAMES.iter().position(|n| n.eq_ignore_ascii_case(name)).map(|i| i as u32)
}

impl ModelConditionFlags {
    pub const fn from_bits(bits: u128) -> Self {
        Self(bits)
    }

    pub fn set(&mut self, bit: u32) {
        self.0 |= 1u128 << bit;
    }

    pub fn clear(&mut self, bit: u32) {
        self.0 &= !(1u128 << bit);
    }

    pub fn test(&self, bit: u32) -> bool {
        self.0 & (1u128 << bit) != 0
    }

    pub fn any(&self) -> bool {
        self.0 != 0
    }

    pub fn bits(&self) -> u128 {
        self.0
    }

    /// The names of every set bit (`ModelConditionFlags::s_bitNameList`).
    pub fn names(&self) -> Vec<&'static str> {
        NAMES.iter().enumerate().filter(|(i, _)| self.0 & (1u128 << i) != 0).map(|(_, n)| *n).collect()
    }
}

/// `BodyDamageType` (`Object/Body/BodyModule.h`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BodyDamageType {
    Pristine,
    Damaged,
    ReallyDamaged,
    Rubble,
}

/// `ActiveBody::calcDamageState` with the `GameData` thresholds.
pub fn calc_damage_state(health: f32, max_health: f32, damaged: f32, really_damaged: f32) -> BodyDamageType {
    let ratio = if max_health > 0.0 { health / max_health } else { 1.0 };
    if ratio > damaged {
        BodyDamageType::Pristine
    } else if ratio > really_damaged {
        BodyDamageType::Damaged
    } else if ratio > 0.0 {
        BodyDamageType::ReallyDamaged
    } else {
        BodyDamageType::Rubble
    }
}

/// `(DOOR_n_OPENING, DOOR_n_CLOSING, DOOR_n_WAITING_OPEN)` for doors 1 to 4 (`NAMES` order), the bits
/// `ProductionUpdate`'s `theOpeningFlags` / `theClosingFlags` / `theWaitingOpenFlags` (`ProductionUpdate.cpp:61-83`) set.
const PRODUCTION_DOOR_BITS: [(u32, u32, u32); 4] = [(18, 19, 20), (22, 23, 24), (26, 27, 28), (30, 31, 32)];

/// The flags of one production door, as `ProductionUpdate::updateDoors` and `update` leave them: opening
/// while `m_doorOpenedFrame` is set, waiting-open while `m_doorWaitOpenFrame` is set, closing while
/// `m_doorClosedFrame` is set (a closed frame left behind by a pop to waiting-open does not show as closing).
fn set_production_door_flags(f: &mut ModelConditionFlags, door: usize, d: &crate::jet_ai::DoorRt) {
    let (opening, closing, waiting) = PRODUCTION_DOOR_BITS[door];
    if d.opened_frame != 0 {
        f.set(opening);
    } else if d.wait_open_frame != 0 {
        f.set(waiting);
    } else if d.closed_frame != 0 {
        f.set(closing);
    }
}

/// Recomputes an object's condition bits from its simulation state (once per frame).
pub(crate) fn derive(o: &Object, now: u32) -> ModelConditionFlags {
    let mut f = ModelConditionFlags::default();

    // ActiveBody damage state -> DAMAGED / REALLYDAMAGED / RUBBLE. Not while under construction:
    // `ActiveBody::internalChangeHealth` skips `evaluateVisualCondition` for OBJECT_STATUS_UNDER_CONSTRUCTION
    // (ActiveBody.cpp, "we do not show visual changes for damage states when things are under construction"),
    // and `DozerAIUpdate.cpp` applies the state once construction completes. A building starts at 1 hp, so
    // the damaged art would otherwise show until its health climbs past each threshold.
    if let (Some(b), false) = (&o.body, o.under_construction) {
        match b.damage_state() {
            BodyDamageType::Pristine => {}
            BodyDamageType::Damaged => f.set(DAMAGED),
            BodyDamageType::ReallyDamaged => f.set(REALLYDAMAGED),
            BodyDamageType::Rubble => f.set(RUBBLE),
        }
    }
    if o.is_dying() {
        f.set(DYING);
    }
    if o.is_ai_moving() {
        f.set(MOVING);
    }
    // `DozerAIUpdate::construct` (`DozerAIUpdate.cpp:1737-1744`) starts a structure `AWAITING_CONSTRUCTION`.
    // The builder reaching the start point swaps that for `PARTIALLY_CONSTRUCTED | ACTIVELY_BEING_CONSTRUCTED`
    // (`DozerAIUpdate.cpp:356-360`), and completion clears all three (`:565-572`).
    if o.under_construction {
        if o.construction_arrived {
            f.set(PARTIALLY_CONSTRUCTED);
            f.set(ACTIVELY_BEING_CONSTRUCTED);
        } else {
            f.set(AWAITING_CONSTRUCTION);
        }
    }
    // `DozerAIUpdate::update` (`DOZER_DO_BUILD_AT_DOCK`, `DozerAIUpdate.cpp:524-528`): the builder shows
    // `ACTIVELY_CONSTRUCTING` while it builds at its dock. Its command clears it (`aiDoCommand`, `:2449`),
    // and the action leaving the dock state (completion or a new order) clears it too.
    if o.build_target.is_some() && o.ai.dozer_action.sub == crate::dozer::DozerActionSub::DoBuildAtDock {
        f.set(ACTIVELY_CONSTRUCTING);
    }
    if o.boxes > 0 {
        f.set(CARRYING);
    }

    // `MissileLauncherBuildingUpdate::switchToState`: the superweapon door conditions.
    if let Some(ml) = &o.superweapon.missile_launcher {
        match ml.door_state {
            crate::superweapon::door_state::OPENING => f.set(DOOR_1_OPENING),
            crate::superweapon::door_state::OPEN => f.set(DOOR_1_WAITING_OPEN),
            crate::superweapon::door_state::WAITING_TO_CLOSE => f.set(DOOR_1_WAITING_TO_CLOSE),
            crate::superweapon::door_state::CLOSING => f.set(DOOR_1_CLOSING),
            _ => {}
        }
    }
    // `ProductionUpdate::update` / `updateDoors`: a factory's door, or an airfield's hangar doors (its
    // parking spaces, of which the first `DOOR_COUNT_MAX` carry flags).
    if let Some(rt) = &o.parking {
        for (i, d) in rt.doors.iter().take(PRODUCTION_DOOR_BITS.len()).enumerate() {
            set_production_door_flags(&mut f, i, d);
        }
    } else if let Some(st) = &o.production {
        set_production_door_flags(&mut f, 0, &st.door);
    }
    // `ParticleUplinkCannonUpdate::setLogicalStatus`: the antenna pack/unpack/deploy conditions.
    if let Some(p) = &o.superweapon.puc {
        match p.status {
            crate::superweapon::puc_status::PREPARING => f.set(UNPACKING),
            crate::superweapon::puc_status::ALMOST_READY
            | crate::superweapon::puc_status::READY_TO_FIRE
            | crate::superweapon::puc_status::FIRING
            | crate::superweapon::puc_status::POSTFIRE => f.set(DEPLOYED),
            crate::superweapon::puc_status::PACKING => f.set(PACKING),
            _ => {}
        }
    }
    // `SpectreGunshipUpdate::friend_enableAfterburners`: the gunship burns in transit.
    if let Some(s) = &o.superweapon.spectre {
        if s.status == crate::superweapon::gunship_status::INSERTING || s.status == crate::superweapon::gunship_status::DEPARTING {
            f.set(JETAFTERBURNER);
        }
    }

    // `RadarUpdate`: `extendRadar` sets RADAR_EXTENDING (`RadarUpdate.cpp:72-75`) and `update` swaps
    // it for RADAR_UPGRADED at the done frame (:104-119).
    if o.behavior.radar_extend_done_frame != 0 {
        f.set(RADAR_EXTENDING);
    }
    if o.behavior.radar_extend_complete {
        f.set(RADAR_UPGRADED);
    }
    // Weapons: `Object::updateDrawModuleWeaponState` / `WeaponSet::getModelConditionForWeaponSlot`.
    let attacking = o.combat.attack.is_some();
    if attacking {
        f.set(ATTACKING);
    }
    let cur_slot = o.combat.attack.as_ref().map(|a| a.slot);
    for i in 0..WEAPON_SLOT_COUNT {
        let Some(w) = o.combat.weapons.weapon(i) else { continue };
        if w.last_fire_frame() == now {
            f.set(FIRING[i]);
            f.set(USING[i]);
            continue;
        }
        if !attacking || cur_slot != Some(i) {
            continue;
        }
        let condition = match w.status(now) {
            WeaponStatus::PreAttack => Some(PREATTACK[i]),
            WeaponStatus::ReloadingClip => Some(RELOADING[i]),
            WeaponStatus::BetweenFiringShots => Some(BETWEEN[i]),
            // A ready weapon that is aiming/firing keeps the "between" pose (srj's special case).
            WeaponStatus::ReadyToFire => o
                .combat
                .attack
                .as_ref()
                .filter(|a| matches!(a.phase, AttackPhase::Aim | AttackPhase::Fire))
                .map(|_| BETWEEN[i]),
            WeaponStatus::OutOfAmmo => None,
        };
        if let Some(bit) = condition {
            f.set(bit);
            f.set(USING[i]);
        }
    }

    // `TurretAI::friend_turnTowardsAngle` sets `MODELCONDITION_TURRET_ROTATE` while it takes a step
    // and clears it when it snaps (`TurretAI.cpp:409-418`).
    if o.combat.turret.as_ref().is_some_and(|t| t.rotating) {
        f.set(TURRET_ROTATE);
    }

    // `FiringTracker` (`FiringTracker.cpp:238-288`): MEAN and FAST mirror the bonus flags it sets, and SLOW is
    // the flag `coolDown` leaves behind. `speedUp`/`coolDown` keep the three exclusive, so this matches the drawable.
    let bonus = o.combat.weapon_bonus_conditions;
    if bonus & (1 << crate::bonus::condition::CONTINUOUS_FIRE_MEAN) != 0 {
        f.set(CONTINUOUS_FIRE_MEAN);
    }
    if bonus & (1 << crate::bonus::condition::CONTINUOUS_FIRE_FAST) != 0 {
        f.set(CONTINUOUS_FIRE_FAST);
    }
    if o.combat.helpers.firing.slow {
        f.set(CONTINUOUS_FIRE_SLOW);
    }

    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damage_states_follow_thresholds() {
        assert_eq!(calc_damage_state(100.0, 100.0, 0.7, 0.35), BodyDamageType::Pristine);
        assert_eq!(calc_damage_state(60.0, 100.0, 0.7, 0.35), BodyDamageType::Damaged);
        assert_eq!(calc_damage_state(20.0, 100.0, 0.7, 0.35), BodyDamageType::ReallyDamaged);
        assert_eq!(calc_damage_state(0.0, 100.0, 0.7, 0.35), BodyDamageType::Rubble);
    }

    #[test]
    fn names_match_bit_indices() {
        assert_eq!(NAMES[DAMAGED as usize], "DAMAGED");
        assert_eq!(NAMES[REALLYDAMAGED as usize], "REALLYDAMAGED");
        assert_eq!(NAMES[RUBBLE as usize], "RUBBLE");
        assert_eq!(NAMES[MOVING as usize], "MOVING");
        assert_eq!(NAMES[DYING as usize], "DYING");
        assert_eq!(NAMES[CARRYING as usize], "CARRYING");
        assert_eq!(NAMES[USING_WEAPON_A as usize], "USING_WEAPON_A");
        assert_eq!(NAMES[ACTIVELY_CONSTRUCTING as usize], "ACTIVELY_CONSTRUCTING");
        assert_eq!(NAMES[CONTINUOUS_FIRE_SLOW as usize], "CONTINUOUS_FIRE_SLOW");
        assert_eq!(NAMES[CONTINUOUS_FIRE_MEAN as usize], "CONTINUOUS_FIRE_MEAN");
        assert_eq!(NAMES[CONTINUOUS_FIRE_FAST as usize], "CONTINUOUS_FIRE_FAST");
        assert_eq!(NAMES[AWAITING_CONSTRUCTION as usize], "AWAITING_CONSTRUCTION");
        // The production door table (`PRODUCTION_DOOR_BITS`) names each door's flags.
        for (i, (opening, closing, waiting)) in PRODUCTION_DOOR_BITS.iter().enumerate() {
            let n = i + 1;
            assert_eq!(bit_for_name(&format!("DOOR_{n}_OPENING")), Some(*opening));
            assert_eq!(bit_for_name(&format!("DOOR_{n}_CLOSING")), Some(*closing));
            assert_eq!(bit_for_name(&format!("DOOR_{n}_WAITING_OPEN")), Some(*waiting));
        }
        let mut f = ModelConditionFlags::default();
        f.set(DAMAGED);
        f.set(MOVING);
        assert_eq!(f.names(), vec!["DAMAGED", "MOVING"]);
        assert!(f.test(DAMAGED) && f.test(MOVING) && !f.test(RUBBLE));
    }
}
