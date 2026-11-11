//! Power (`Common/RTS/Energy.cpp`, `Player::getEnergy`) and the global economy rules the sim needs
//! (`GlobalData.cpp`).
//!
//! Energy is a per-player tally of the completed objects' `EnergyProduction`: positive values
//! produce, negative values consume. The supply ratio is `production / consumption` (or
//! `production` when nothing consumes), and a shortfall slows production by the engine's factor
//! (`ThingTemplate::calcTimeToBuild`).
//!
//! The tally is recomputed on demand rather than maintained incrementally like the engine's
//! `Energy`. It applies the same rules: a disabled producer contributes nothing
//! (`Object::friend_adjustPowerForPlayer` Object.cpp:3845 and `Object::onDisabledEdge`
//! Object.cpp:3895), and an active `PowerPlantUpgrade` / `OverchargeBehavior` adds the template's
//! `EnergyBonus`. Disabled consumers still count, so the producer-only rule cannot feed back.

use crate::logic::GameLogic;
use crate::object::PlayerIndex;
use crate::template::ObjectTemplate;

/// Global rules from the `GameData` block, set by the adapter (`Game::from_map`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GameRules {
    /// `ValuePerSupplyBox`: money deposited per supply box.
    pub value_per_supply_box: u32,
    /// `AllowedHeightVariationForBuilding`: build flatness tolerance.
    pub allowed_height_variation_for_building: f32,
    /// `LowEnergyPenaltyModifier`.
    pub low_energy_penalty_modifier: f32,
    /// `MinLowEnergyProductionSpeed`.
    pub min_low_energy_production_speed: f32,
    /// `MaxLowEnergyProductionSpeed`.
    pub max_low_energy_production_speed: f32,
    /// `GameData` `Gravity` (per frame^2), used by [`crate::physics`].
    pub gravity: f32,
    /// `GameData` `GroundStiffness` (`PhysicsBehavior::handleBounce`).
    pub ground_stiffness: f32,
    /// `GameData` `StructureStiffness` (`PhysicsBehavior::onCollide` bounces off immobile things).
    pub structure_stiffness: f32,
    /// `UnitDamagedThreshold`: health ratio above which the body is `BODY_PRISTINE`.
    pub unit_damaged_threshold: f32,
    /// `UnitReallyDamagedThreshold`: health ratio above which the body is `BODY_DAMAGED`.
    pub unit_really_damaged_threshold: f32,
    /// `GlobalData::m_movementPenaltyDamageState` (`MovementPenaltyDamageState`, `GlobalData.cpp:411`): from this body
    /// state down (`!(condition < state)`, `BodyModule.h:103`) a locomotor uses its `*Damaged` values.
    pub movement_penalty_damage_state: crate::condition::BodyDamageType,
    /// `GlobalData::BaseRegenHealthPercentPerSecond` (base buildings heal this fraction/s).
    pub base_regen_health_percent_per_second: f32,
    /// `GlobalData::BaseRegenDelay` (frames before regen resumes after damage).
    pub base_regen_delay: u32,
    /// `GlobalData::MaxTunnelCapacity`: how many units one player's tunnel network can hold.
    pub max_tunnel_capacity: i32,
}

impl Default for GameRules {
    /// The engine's `GlobalData` constructor defaults (all zero except `ValuePerSupplyBox`); the
    /// adapter overwrites them from the shipped `GameData` block. Zero rules mean no penalty.
    fn default() -> Self {
        GameRules {
            value_per_supply_box: 100,
            allowed_height_variation_for_building: 0.0,
            low_energy_penalty_modifier: 0.0,
            min_low_energy_production_speed: 0.0,
            max_low_energy_production_speed: 0.0,
            // The shipped `GameData.ini` says `Gravity = -64` (dist/sec²); `INI::parseAccelerationReal`
            // converts it to frames: -64 / 30².
            gravity: -64.0 / 900.0,
            ground_stiffness: 0.5,
            structure_stiffness: 0.5,
            // `GlobalData.cpp` constructor defaults; the adapter overwrites from the shipped block.
            unit_damaged_threshold: 0.5,
            unit_really_damaged_threshold: 0.1,
            // `GlobalData.cpp:1032`.
            movement_penalty_damage_state: crate::condition::BodyDamageType::ReallyDamaged,
            base_regen_health_percent_per_second: 0.0,
            base_regen_delay: 0,
            max_tunnel_capacity: 0,
        }
    }
}

impl GameRules {
    /// The build-speed multiplier from `ThingTemplate::calcTimeToBuild` (power/handicap factors at
    /// 1.0 otherwise). `1.0` = full speed; lower is slower. Unset rules (all zero) mean no penalty.
    pub fn low_power_penalty(&self, supply_ratio: f32) -> f32 {
        if self.low_energy_penalty_modifier <= 0.0 || self.max_low_energy_production_speed <= 0.0 {
            return 1.0;
        }
        let energy = if supply_ratio > 1.0 { 1.0 } else { supply_ratio };
        let mut short = (1.0 - energy) * self.low_energy_penalty_modifier;
        if short < 0.0 {
            short = 0.0;
        }
        let mut rate = 1.0 - short;
        if rate < self.min_low_energy_production_speed {
            rate = self.min_low_energy_production_speed;
        }
        if energy < 1.0 && rate > self.max_low_energy_production_speed {
            rate = self.max_low_energy_production_speed;
        }
        if rate <= 0.0 {
            rate = 0.01;
        }
        rate
    }
}

impl GameLogic {
    /// `(production, consumption)` from every completed object of `player`
    /// (`Energy::objectEnteringInfluence`). A disabled producer is skipped
    /// (`Object::friend_adjustPowerForPlayer`) and an active `PowerPlantUpgrade` /
    /// `OverchargeBehavior` adds `EnergyBonus` (`Object::onDisabledEdge`); consumers always count.
    pub fn energy_tally(&self, player: PlayerIndex) -> (i32, i32) {
        let (mut production, mut consumption) = (0i32, 0i32);
        for o in self.objects.values() {
            if o.destroyed || o.player != player || o.under_construction {
                continue;
            }
            let e = o.template.energy_production;
            if e > 0 {
                if o.is_disabled_producer() {
                    continue;
                }
                production += e + o.active_energy_bonus();
            } else if e < 0 {
                consumption += -e;
            }
        }
        (production, consumption)
    }

    /// `Energy::getEnergySupplyRatio` (`Energy.cpp:78`). A sabotaged power grid has no ratio.
    pub fn energy_supply_ratio(&self, player: PlayerIndex) -> f32 {
        if self.power_sabotaged_till.get(&player).is_some_and(|&f| self.frame < f) {
            return 0.0;
        }
        let (production, consumption) = self.energy_tally(player);
        if consumption == 0 {
            production as f32
        } else {
            production as f32 / consumption as f32
        }
    }

    /// `Energy::hasSufficientPower` (`Energy.cpp:95`). A sabotaged power grid has no power.
    /// Whether `player`'s grid is sabotaged this frame (`Energy::getProduction` returns 0 while
    /// `m_powerSabotagedTillFrame` is ahead of the frame). Read-only; used by the snapshot writer.
    pub fn is_power_sabotaged(&self, player: PlayerIndex) -> bool {
        self.power_sabotaged_till.get(&player).is_some_and(|&f| self.frame < f)
    }

    pub fn has_sufficient_power(&self, player: PlayerIndex) -> bool {
        if self.power_sabotaged_till.get(&player).is_some_and(|&f| self.frame < f) {
            return false;
        }
        let (production, consumption) = self.energy_tally(player);
        production >= consumption
    }

    /// `ThingTemplate::calcTimeToBuild` in frames, with the low-power penalty (handicap / faction
    /// modifiers are 1.0).
    pub(crate) fn effective_build_time_frames(&self, t: &ObjectTemplate, player: PlayerIndex) -> u32 {
        let penalty = self.rules.low_power_penalty(self.energy_supply_ratio(player));
        (t.build_time_frames as f32 / penalty) as u32
    }
}
