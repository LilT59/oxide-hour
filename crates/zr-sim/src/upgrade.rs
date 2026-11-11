//! Upgrades and the object/player upgrade sets (`Object::giveUpgrade` /
//! `updateUpgradeModules`, `UpgradeMux::testUpgradeConditions`, `Player::addUpgrade`).
//!
//! An upgrade module is activated when any of its `TriggeredBy` names is in the object's or its
//! owner's upgrade set (and no `ConflictsWith` name is). Active modules contribute condition flags
//! (`ArmorUpgrade` / `WeaponSetUpgrade` set the `PLAYER_UPGRADE` bit), change max health
//! (`MaxHealthUpgrade`), and the object's armor/weapon set is chosen as the most specific set whose
//! conditions are satisfied (`findBestInfo`). `LocomotorSetUpgrade` sets its flag but does not swap
//! the locomotor yet.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::armor::ArmorTemplate;
use crate::body::MaxHealthChangeType;
use crate::logic::GameLogic;
use crate::object::{ObjectId, PlayerIndex};
use crate::weapon::WeaponSetTemplate;

/// The set-condition bits the sim needs, supplied by the adapter from `zr-templates`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SetFlagBits {
    pub player_upgrade: u32,
    pub veteran: u32,
    pub elite: u32,
    pub hero: u32,
}

/// `UpgradeTemplate::calcCostToBuild` / `calcTimeToBuild` (sim copy).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UpgradeInfo {
    pub cost: u32,
    pub build_time_frames: u32,
}

impl SetFlagBits {
    /// The condition flag for a level (`onVeterancyLevelChanged` sets only that level's bit).
    pub fn veterancy(&self, level: crate::experience::VeterancyLevel) -> u32 {
        match level {
            crate::experience::VeterancyLevel::Regular => 0,
            crate::experience::VeterancyLevel::Veteran => self.veteran,
            crate::experience::VeterancyLevel::Elite => self.elite,
            crate::experience::VeterancyLevel::Heroic => self.hero,
        }
    }

    /// All three veterancy bits: `onVeterancyLevelChanged` clears these and sets only the new one.
    pub fn veterancy_mask(&self) -> u32 {
        self.veteran | self.elite | self.hero
    }
}

/// What an active upgrade module does.
#[derive(Clone, Debug)]
pub enum UpgradeEffect {
    /// `ArmorUpgrade`: OR `flag_bit` into the object's armor-set flags.
    Armor { flag_bit: u32 },
    /// `WeaponSetUpgrade`: OR `flag_bit` into the object's weapon-set flags.
    WeaponSet { flag_bit: u32 },
    /// `LocomotorSetUpgrade`: flag only (the locomotor is not swapped yet).
    Locomotor { flag_bit: u32 },
    /// `MaxHealthUpgrade`.
    MaxHealth { add_max_health: f32, change_type: MaxHealthChangeType },
    /// `ObjectCreationUpgrade`: run this OCL once when the upgrade activates.
    ObjectCreation { ocl: Option<Arc<crate::ocl::OclTemplate>> },
    /// `CommandSetUpgrade` (the sim has no command-set model; the client uses the names).
    CommandSet { command_set: String, command_set_alt: String, trigger_alt: String },
    /// `ExperienceScalarUpgrade`.
    ExperienceScalar { add_xp_scalar: f32 },
    /// `WeaponBonusUpgrade` (`WeaponBonusUpgrade.cpp:86`): sets the weapon-bonus `PLAYER_UPGRADE`
    /// condition on the object, which selects no weapon set.
    WeaponBonus,
    /// `ModelConditionUpgrade`: OR a model-condition bit (viewer state).
    ModelCondition { flag: Option<String> },
    /// `PowerPlantUpgrade`: while active, adds the template's `EnergyBonus` to this object's power
    /// (`PowerPlantUpgrade.cpp` `addPowerBonus`).
    PowerPlant,
    /// `UnpauseSpecialPowerUpgrade` (`UnpauseSpecialPowerUpgrade.cpp:84`): once, unpause each
    /// `SpecialPowerModule` of `power` on the object.
    UnpauseSpecialPower { power: String },
    /// `StealthUpgrade` (`StealthUpgrade.cpp:51-70`): once, set `OBJECT_STATUS_CAN_STEALTH` on the
    /// object (and on a `KINDOF_SPAWNS_ARE_THE_WEAPONS` spawner's slaves).
    Stealth,
    /// `RadarUpgrade` (`RadarUpgrade.cpp:128-143`): once, `Player::addRadar(DisableProof)` and
    /// `RadarUpdate::extendRadar` on this object's update module.
    Radar { disable_proof: bool },
    /// `GrantScienceUpgrade` (`GrantScienceUpgrade.cpp:85-99`): once, `Player::grantScience`.
    GrantScience { science: String },
    /// `CostModifierUpgrade` (`CostModifierUpgrade.cpp:166-173`): once, add the
    /// `(EffectKindOf, Percentage)` entry to the player's `m_kindOfPercentProductionChangeList`
    /// (`Player::addKindOfProductionCostChange`), read by `ThingTemplate::calcCostToBuild`.
    CostModifier { kind_of: u64, percentage: f32 },
    /// `PassengersFireUpgrade` (`PassengersFireUpgrade.cpp:52-66`): once,
    /// `contain->setPassengerAllowedToFire(TRUE)`. `GarrisonContain::isPassengerAllowedToFire`
    /// ignores the flag (a garrison always allows it), so only the other contain classes change.
    PassengersFire,
    /// `ReplaceObjectUpgrade` (`ReplaceObjectUpgrade.cpp:71-140`): once, destroy this object and create
    /// `template` at the same transform, team and orientation (full health, a fresh id).
    ReplaceObject { template: Option<Arc<crate::template::ObjectTemplate>> },
    /// `SubObjectsUpgrade` (`SubObjectsUpgrade.cpp:85-130`): client-only show/hide of the model's
    /// sub-objects. The sim only carries the module so `affectedByUpgrade` / `updateUpgradeModules`
    /// see the upgrade (the viewer reads the object's applied upgrades and the module's name lists).
    SubObjects,
}

/// A typed upgrade module attached to an object template.
#[derive(Clone, Debug)]
pub struct SimUpgradeModule {
    pub triggered_by: Vec<String>,
    pub conflicts_with: Vec<String>,
    pub removes_upgrades: Vec<String>,
    /// `UpgradeMuxData::m_requiresAllActivationUpgrades` (`RequiresAllTriggers`): every `triggered_by` name
    /// must be held, not just one (`UpgradeMux::testUpgradeConditions` / `wouldUpgrade`, UpgradeModule.cpp:124-150).
    pub requires_all_triggers: bool,
    pub effect: UpgradeEffect,
}

/// Pick the most specific armor set whose conditions are all satisfied (`findBestInfo`).
pub fn select_armor_set(sets: &[(u32, Option<Arc<ArmorTemplate>>)], flags: u32) -> Option<Option<Arc<ArmorTemplate>>> {
    let mut best: Option<(u32, &Option<Arc<ArmorTemplate>>)> = None;
    for (cond, armor) in sets {
        if cond & !flags == 0 {
            let specificity = cond.count_ones();
            if best.is_none_or(|(b, _)| specificity > b) {
                best = Some((specificity, armor));
            }
        }
    }
    best.map(|(_, a)| a.clone())
}

/// Pick the most specific weapon set whose conditions are all satisfied (`findBestInfo`).
pub fn select_weapon_set(sets: &[(u32, WeaponSetTemplate)], flags: u32) -> Option<&WeaponSetTemplate> {
    let mut best: Option<(u32, &WeaponSetTemplate)> = None;
    for (cond, set) in sets {
        if cond & !flags == 0 {
            let specificity = cond.count_ones();
            if best.is_none_or(|(b, _)| specificity > b) {
                best = Some((specificity, set));
            }
        }
    }
    best.map(|(_, s)| s)
}

impl GameLogic {
    // ---- upgrade sets ----------------------------------------------------------------------

    /// The object's own upgrade set.
    pub fn object_upgrades(&self, id: ObjectId) -> impl Iterator<Item = &String> {
        self.find_object(id).into_iter().flat_map(|o| o.upgrades.iter())
    }

    /// The player's upgrade set.
    pub fn player_upgrades(&self, player: PlayerIndex) -> impl Iterator<Item = &String> {
        self.player_upgrade_set(player).into_iter().flat_map(|s| s.iter())
    }

    /// `Object::giveUpgrade`: add to the object set, apply `RemovesUpgrades`, and update modules.
    pub fn give_upgrade(&mut self, id: ObjectId, name: &str) {
        let removes: Vec<String> = self
            .find_object(id)
            .and_then(|o| o.template.upgrade_modules.iter().find(|m| m.triggered_by.iter().any(|n| n == name)))
            .map(|m| m.removes_upgrades.clone())
            .unwrap_or_default();
        let Some(o) = self.objects.get_mut(&id) else { return };
        o.upgrades.insert(name.to_string());
        for r in removes {
            o.upgrades.remove(&r);
        }
        self.update_upgrade_modules(id);
    }

    /// `Player::addUpgrade`: add to the player set and apply to every owned object.
    pub fn apply_player_upgrade(&mut self, player: PlayerIndex, name: &str) {
        self.player_upgrade_set_mut(player).insert(name.to_string());
        let owned: Vec<ObjectId> = self.objects.values().filter(|o| !o.destroyed && o.player == player).map(|o| o.id).collect();
        for id in owned {
            self.update_upgrade_modules(id);
        }
    }

    /// `Object::updateUpgradeModules`: recompute the condition flags and apply effects.
    pub fn update_upgrade_modules(&mut self, id: ObjectId) {
        // `Object::updateUpgradeModules` (Object.cpp:2479-2488): no upgrade runs while the object is
        // under construction (the three places that clear that status re-update it, e.g. the dozer's
        // completion in `build.rs`) or already destroyed (the fake GLA building patch).
        match self.find_object(id) {
            Some(o) if !o.under_construction && !o.is_destroyed() => {}
            _ => return,
        }
        // `AutoHealBehavior` is an `UpgradeMux` too (`Object::updateUpgradeModules` -> `attemptUpgrade`,
        // UpgradeModule.cpp:111-150): its activation lives with the sleepy family (`sleepy_families.rs`).
        self.auto_heal_upgrade_event(id);
        let Some(o) = self.find_object(id) else { return };
        let modules = o.template.upgrade_modules.clone();
        let player = o.player;
        let obj_upgrades = o.upgrades.clone();
        let player_upgrades = self.player_upgrade_set(player).cloned().unwrap_or_default();
        let level = o.experience.level;
        let armor_bits = o.template.combat.armor_flag_bits;
        let weapon_bits = o.template.combat.weapon_flag_bits;

        let held = |n: &String| obj_upgrades.contains(n) || player_upgrades.contains(n);
        // `UpgradeMux::testUpgradeConditions` (UpgradeModule.cpp:169): ANY trigger held, or with `RequiresAllTriggers` every one.
        let active = |m: &SimUpgradeModule| {
            let trig = if m.requires_all_triggers {
                !m.triggered_by.is_empty() && m.triggered_by.iter().all(held)
            } else {
                m.triggered_by.iter().any(held)
            };
            let conflict = m.conflicts_with.iter().any(held);
            trig && !conflict
        };
        // Retail never rebuilds the flag sets: `onVeterancyLevelChanged` (`Object.cpp:3150-3185`)
        // clears the three veterancy bits and sets the new one, and each upgrade only sets its own bit
        // (`WeaponSetUpgrade.cpp:56`, `ArmorUpgrade.cpp:98`). Everything else (`CARBOMB`,
        // `CRATEUPGRADE_*`, `VEHICLE_HIJACK`, `MINE_CLEARING_DETAIL`, riders, armor crate and second-life
        // bits) is left alone. An upgrade bit is OR-only here: the retail upgrade runs once and nothing
        // clears its flag.
        let mut armor_add = 0u32;
        let mut weapon_add = 0u32;
        let mut power_plant = false;
        let mut weapon_bonus_upgraded = false;
        for m in &modules {
            if !active(m) {
                continue;
            }
            match m.effect {
                UpgradeEffect::Armor { flag_bit } => armor_add |= flag_bit,
                UpgradeEffect::WeaponSet { flag_bit } => weapon_add |= flag_bit,
                UpgradeEffect::Locomotor { .. } => {
                    // `LocomotorSetUpgrade::upgradeImplementation` (LocomotorSetUpgrade.cpp:54-56): only an object
                    // with an AI. Then `AIUpdateInterface::setLocomotorUpgrade(true)`: `SET_NORMAL` becomes
                    // `SET_NORMAL_UPGRADED` (re-choosing the current set).
                    if self.find_object(id).is_some_and(|o| o.ai_handle.is_some()) {
                        let cur = self.objects.get_mut(&id).map(|o| {
                            o.loco_upgraded = true;
                            o.locomotor_set
                        });
                        if cur == Some(crate::template::LocomotorSet::Normal) {
                            self.set_locomotor_set(id, crate::template::LocomotorSet::NormalUpgraded);
                        }
                    }
                }
                UpgradeEffect::MaxHealth { add_max_health, change_type } => {
                    let key = format!("maxhealth:{add_max_health}:{change_type:?}");
                    let already = self.objects.get(&id).is_some_and(|o| o.applied_upgrades.contains(&key));
                    if !already {
                        if let Some(o) = self.objects.get_mut(&id) {
                            o.applied_upgrades.insert(key);
                            if let Some(b) = o.body.as_mut() {
                                b.change_max_health(add_max_health, change_type);
                            }
                        }
                    }
                }
                UpgradeEffect::ObjectCreation { ref ocl } => {
                    let Some(ocl) = ocl else { continue };
                    let key = format!("ocl:{}", ocl.name);
                    if self.objects.get(&id).is_some_and(|o| o.applied_upgrades.contains(&key)) {
                        continue;
                    }
                    let ocl = ocl.clone();
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.applied_upgrades.insert(key);
                    }
                    self.run_ocl(&ocl, id);
                }
                UpgradeEffect::ExperienceScalar { add_xp_scalar } => {
                    let key = format!("xpscalar:{add_xp_scalar}");
                    if self.objects.get(&id).is_some_and(|o| o.applied_upgrades.contains(&key)) {
                        continue;
                    }
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.applied_upgrades.insert(key);
                        o.experience.experience_scalar += add_xp_scalar;
                    }
                }
                UpgradeEffect::WeaponBonus => weapon_bonus_upgraded = true,
                UpgradeEffect::ModelCondition { ref flag } => {
                    if let Some(bit) = flag.as_deref().and_then(crate::condition::bit_for_name) {
                        if let Some(o) = self.objects.get_mut(&id) {
                            o.upgrade_conditions |= 1u128 << bit;
                        }
                    }
                }
                UpgradeEffect::CommandSet { .. } => {
                    // The sim has no command-set model; the client reads the names from the module.
                }
                UpgradeEffect::PowerPlant => power_plant = true,
                UpgradeEffect::Stealth => {
                    // `UpgradeModule::attemptUpgrade` runs `upgradeImplementation` once per object
                    // (`m_upgradeExecuted`): a later upgrade must not set `CAN_STEALTH` again after a
                    // temporary grant was switched off.
                    let key = "stealth".to_string();
                    if self.objects.get(&id).is_some_and(|o| o.applied_upgrades.contains(&key)) {
                        continue;
                    }
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.applied_upgrades.insert(key);
                    }
                    self.stealth_upgrade_activate(id);
                }
                UpgradeEffect::UnpauseSpecialPower { ref power } => {
                    // `UpgradeModule::attemptUpgrade` runs `upgradeImplementation` once per object.
                    let key = format!("unpause:{power}");
                    if self.objects.get(&id).is_some_and(|o| o.applied_upgrades.contains(&key)) {
                        continue;
                    }
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.applied_upgrades.insert(key);
                    }
                    // `sp->pauseCountdown(FALSE)` for each module whose template is `power`
                    // (UnpauseSpecialPowerUpgrade.cpp:92-94). `startsReady()` is false under
                    // `RETAIL_COMPATIBLE_CRC` (SpecialPowerModule.cpp:382), so no ready-frame reset.
                    for (name, _) in self.special_power_modules(id) {
                        if name == *power {
                            self.pause_countdown(id, &name, false);
                        }
                    }
                }
                UpgradeEffect::Radar { disable_proof } => {
                    // `UpgradeModule::attemptUpgrade` runs `upgradeImplementation` once per object
                    // (`m_upgradeExecuted`); `RadarUpgrade::onDelete` clears it again.
                    let key = format!("radar:{disable_proof}");
                    if self.objects.get(&id).is_some_and(|o| o.applied_upgrades.contains(&key)) {
                        continue;
                    }
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.applied_upgrades.insert(key);
                    }
                    self.radar_upgrade_activate(id, disable_proof);
                }
                UpgradeEffect::GrantScience { ref science } => {
                    // Once per object: `GrantScienceUpgrade` caches `m_scienceType` and
                    // `m_upgradeExecuted` stops a second grant.
                    let key = format!("science:{science}");
                    if self.objects.get(&id).is_some_and(|o| o.applied_upgrades.contains(&key)) {
                        continue;
                    }
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.applied_upgrades.insert(key);
                    }
                    if let Some(player) = self.find_object(id).map(|o| o.player) {
                        self.grant_science(player, science);
                    }
                }
                UpgradeEffect::CostModifier { kind_of, percentage } => {
                    // Once per object; `CostModifierUpgrade::onDelete` removes the entry again.
                    let key = format!("costmod:{kind_of}:{percentage}");
                    if self.objects.get(&id).is_some_and(|o| o.applied_upgrades.contains(&key)) {
                        continue;
                    }
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.applied_upgrades.insert(key);
                    }
                    if let Some(player) = self.find_object(id).map(|o| o.player) {
                        self.add_production_cost_change(player, kind_of, percentage);
                    }
                }
                UpgradeEffect::PassengersFire => {
                    // `contain->setPassengerAllowedToFire(TRUE)` (PassengersFireUpgrade.cpp:60-64).
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.contain_passengers_allowed_to_fire = true;
                    }
                }
                UpgradeEffect::SubObjects => {
                    // Client-only (SubObjectsUpgrade.cpp:104-129): nothing to do in the sim.
                }
                UpgradeEffect::ReplaceObject { ref template } => {
                    let Some(template) = template.clone() else { continue };
                    // `ReplaceObjectUpgrade::upgradeImplementation` destroys this object and makes the
                    // replacement, which `Object::updateUpgradeModules`'s status guard would skip from
                    // here on: stop the loop for the dying object.
                    self.replace_object_upgrade(id, &template);
                    return;
                }
            }
        }

        // Combine with the flags as they are now: an effect above may have changed them.
        let Some(o) = self.objects.get_mut(&id) else { return };
        // `PowerPlantUpgrade::upgradeImplementation` runs once, when the upgrade executes, and extends the rods of
        // every power plant interface (`PowerPlantUpgrade.cpp:110-128`): the first time the flag turns on.
        let newly_power_plant = power_plant && !o.behavior.power_plant_upgraded;
        o.behavior.power_plant_upgraded = power_plant;
        o.weapon_bonus_player_upgrade |= weapon_bonus_upgraded;
        let armor_flags = (o.armor_set_flags & !armor_bits.veterancy_mask()) | armor_bits.veterancy(level) | armor_add;
        let weapon_flags = (o.weapon_set_flags & !weapon_bits.veterancy_mask()) | weapon_bits.veterancy(level) | weapon_add;
        self.replace_armor_set_flags(id, armor_flags);
        self.replace_weapon_set_flags(id, weapon_flags);
        if newly_power_plant {
            self.extend_power_plant_rods(id, true);
        }
    }

    // ---- upgrade module effects ----------------------------------------------------------------

    /// `RadarUpgrade::upgradeImplementation` (`RadarUpgrade.cpp:128-143`): give the controlling
    /// player a radar facility and extend this object's `RadarUpdate`.
    fn radar_upgrade_activate(&mut self, id: ObjectId, disable_proof: bool) {
        let Some(player) = self.find_object(id).map(|o| o.player) else { return };
        if let Some(p) = self.players.get_mut(&player) {
            p.add_radar(disable_proof);
        }
        self.extend_radar(id);
    }

    /// `RadarUpdate::extendRadar` (`RadarUpdate.cpp:66-83`): set the model condition (derived from
    /// `radar_extend_done_frame`), the frame the extension completes on
    /// (`frame + RadarExtendTime`; the INI duration is in frames) and `m_radarActive` (which only
    /// `isRadarActive` reads; the sim has no reader).
    fn extend_radar(&mut self, id: ObjectId) {
        let now = self.frame;
        // `getObject()->findUpdateModule(NAMEKEY("RadarUpdate"))`: no module, no extension.
        let Some((extend, has_module)) = self
            .find_object(id)
            .map(|o| (o.template.radar_extend_time, o.template.update_module_order.contains(&crate::template::SleepyModuleKind::Radar)))
        else {
            return;
        };
        if !has_module {
            return;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.behavior.radar_extend_done_frame = now.saturating_add(extend as u32);
            // `extendRadar` does not reset `m_extendComplete`: a second extension on an already
            // extended object never reports done, as in C++.
        }
    }

    /// `Player::grantScience` (`Player.cpp:2655-2664`): only a science with `IsGrantable = Yes`
    /// can be granted (the shipped `Upgrade_AmericaMOAB` grants `SCIENCE_MOAB`).
    pub fn grant_science(&mut self, player: PlayerIndex, name: &str) -> bool {
        if !self.science_templates.get(name).is_some_and(|s| s.grantable) {
            return false;
        }
        self.add_science(player, name);
        true
    }

    /// `Player::addKindOfProductionCostChange` (`CostModifierUpgrade.cpp:166-173`).
    pub fn add_production_cost_change(&mut self, player: PlayerIndex, kind_of: u64, percentage: f32) {
        if let Some(p) = self.players.get_mut(&player) {
            p.add_production_cost_change(kind_of, percentage);
        }
    }

    /// `Player::removeKindOfProductionCostChange` (`CostModifierUpgrade::onDelete`, CostModifierUpgrade.cpp:118-135).
    pub fn remove_production_cost_change(&mut self, player: PlayerIndex, kind_of: u64, percentage: f32) {
        if let Some(p) = self.players.get_mut(&player) {
            p.remove_production_cost_change(kind_of, percentage);
        }
    }

    /// `Player::getProductionCostChangeBasedOnKindOf` (`Player.cpp:3913-3932`).
    pub fn production_cost_change_based_on_kind_of(&self, player: PlayerIndex, kind_of: u64) -> f32 {
        self.players.get(&player).map_or(1.0, |p| p.production_cost_change_based_on_kind_of(kind_of))
    }

    /// `ThingTemplate::calcCostToBuild` (`Core/.../ThingTemplate.cpp:1563-1572`): the build cost
    /// times the player's kind-of cost modifier. The per-template `ProductionCostChange` and the
    /// handicap multiplier are still 1.0, as before.
    pub fn calc_cost_to_build(&self, player: PlayerIndex, t: &crate::template::ObjectTemplate) -> u32 {
        let modifier = self.production_cost_change_based_on_kind_of(player, t.kind_of);
        (t.build_cost.max(0) as f32 * modifier) as u32
    }

    /// `Player::hasRadar` (`Player.cpp:3270-3278`): whether the player's minimap has radar.
    pub fn player_has_radar(&self, player: PlayerIndex) -> bool {
        self.players.get(&player).is_some_and(|p| p.has_radar())
    }

    /// `ReplaceObjectUpgrade::upgradeImplementation` (`ReplaceObjectUpgrade.cpp:71-140`): remove this
    /// object from the pathfind map, destroy it, create `template` at the same transform matrix with
    /// the same team (`TheThingFactory->newObject(template, myTeam)`), then run the structure
    /// completion the engine calls on the replacement (`Player::onStructureConstructionComplete`,
    /// Player.cpp:1632-1653). The pitch/roll of the transform matrix are not modelled (the sim
    /// objects carry yaw only; every shipped user is a GLA fake building).
    fn replace_object_upgrade(&mut self, id: ObjectId, template: &std::sync::Arc<crate::template::ObjectTemplate>) {
        let Some(o) = self.objects.get(&id) else { return };
        let (pos, angle, player, team) = (o.position, o.angle, o.player, o.team.clone());
        // `TheAI->pathfinder()->removeObjectFromPathfindMap(me)` then `destroyObject(me)` (:97-101):
        // the destroy runs the module deletes and removes the footprint.
        self.destroy_object(id);
        let new = self.create_object(template, pos, angle, player);
        if let Some(team) = team {
            if self.find_object(new).is_some_and(|o| o.team.as_deref() != Some(team.as_str())) {
                self.set_object_team_key(new, &team);
            }
        }
        // `addObjectToPathfindMap(replacementObject)` (:105) and the completion's own
        // remove/add (`Player::onStructureConstructionComplete`, Player.cpp:1640-1641).
        if let Some(o) = self.objects.get(&new) {
            let (pos, angle, tmpl) = (o.position, o.angle, o.template.clone());
            self.pathfinder.classify_object_footprint(&self.terrain, new, &tmpl, &pos, angle, false);
            self.pathfinder.classify_object_footprint(&self.terrain, new, &tmpl, &pos, angle, true);
        }
        self.score_structure_completed(new);
        self.ai_on_structure_produced(new);
        // `for (BehaviorModule** m ...) create->onBuildComplete()` (`ReplaceObjectUpgrade.cpp:109-115`):
        // the finished object's create modules run once more (`SpecialPowerCreate` starts the powers,
        // `GrantUpgradeCreate` grants its upgrade); then the upgrade modules
        // (`Object::updateUpgradeModules`, Object.cpp:2479).
        self.create_modules_on_build_complete(new);
        self.update_upgrade_modules(new);
    }

    /// `for (BehaviorModule** m ...) create->onBuildComplete()` for the create modules the port
    /// models: `SpecialPowerCreate` (`SpecialPowerCreate.cpp:57`) and `GrantUpgradeCreate`
    /// (`GrantUpgradeCreate.cpp:117-139`). The engine calls this loop on a finished object -- a
    /// dozer-built structure (`DozerAIUpdate.cpp:599`), a factory's produced unit
    /// (`ProductionUpdate.cpp:800`), a replacement (`ReplaceObjectUpgrade.cpp:114`), a starting
    /// building (`GameLogic.cpp:511`) and a map object (`GameLogic.cpp:1953`). Each `CreateModule`
    /// runs it once (`m_needToRunOnBuildComplete`); the modelled effects are idempotent, so the port
    /// does not keep that flag.
    pub fn create_modules_on_build_complete(&mut self, id: ObjectId) {
        if self.objects.get(&id).is_some_and(|o| o.template.special_power_create) {
            self.on_special_power_creation(id);
            // `CashBountyPower::onSpecialPowerCreation` (CashBountyPower.cpp:172), no science test.
            self.cash_bounty_on_special_power_creation(id);
        }
        self.grant_upgrade_create(id, true);
    }

    /// `GrantUpgradeCreate::onCreate` (`GrantUpgradeCreate.cpp:82-115`) and `onBuildComplete`
    /// (`:117-139`). `onCreate` grants only when `ExemptStatus` names `UNDER_CONSTRUCTION` and the
    /// object does not have that status; `onBuildComplete` always grants. A player-type upgrade goes
    /// to the controlling player (`Player::addUpgrade`, `UPGRADE_STATUS_COMPLETE`), an object-type
    /// one to the object itself (`Object::giveUpgrade`, Object.cpp:4535). The engine's
    /// `player->getAcademyStats()->recordUpgrade` call (`:111`) has no counterpart in the sim.
    pub(crate) fn grant_upgrade_create(&mut self, id: ObjectId, build_complete: bool) {
        let Some(o) = self.find_object(id) else { return };
        let under_construction = o.under_construction;
        let player = o.player;
        let modules = o.template.grant_upgrade_create.clone();
        for m in modules {
            let grant = build_complete || (m.exempt_under_construction && !under_construction);
            if !grant {
                continue;
            }
            if m.player_upgrade {
                self.apply_player_upgrade(player, &m.upgrade);
            } else {
                self.give_upgrade(id, &m.upgrade);
            }
        }
    }

    /// `Object::onDestroy`'s module deletes (`Object.cpp:777-783`) for the two upgrade modules whose
    /// `onDelete` undoes player state: `RadarUpgrade::onDelete` (`RadarUpgrade.cpp:72-92`) and
    /// `CostModifierUpgrade::onDelete` (`CostModifierUpgrade.cpp:118-135`).
    pub(crate) fn upgrade_module_on_delete(&mut self, id: ObjectId) {
        let Some(o) = self.find_object(id) else { return };
        let disabled = o.is_disabled();
        // `RadarUpgrade::onDelete` returns early while the object is disabled; `CostModifierUpgrade`
        // has no such test.
        let player = o.player;
        let radar: Vec<bool> = if disabled {
            Vec::new()
        } else {
            o.template
                .upgrade_modules
                .iter()
                .filter_map(|m| match m.effect {
                    UpgradeEffect::Radar { disable_proof } if o.applied_upgrades.contains(&format!("radar:{disable_proof}")) => Some(disable_proof),
                    _ => None,
                })
                .collect()
        };
        let costs: Vec<(u64, f32)> = o
            .template
            .upgrade_modules
            .iter()
            .filter_map(|m| match m.effect {
                UpgradeEffect::CostModifier { kind_of, percentage }
                    if o.applied_upgrades.contains(&format!("costmod:{kind_of}:{percentage}")) =>
                {
                    Some((kind_of, percentage))
                }
                _ => None,
            })
            .collect();
        for disable_proof in radar {
            if let Some(p) = self.players.get_mut(&player) {
                p.remove_radar(disable_proof);
            }
        }
        for (kind_of, percentage) in costs {
            self.remove_production_cost_change(player, kind_of, percentage);
        }
    }

    /// `Object::onCapture`'s upgrade-module half (`Object.cpp:4585-4605`): `RadarUpgrade::onCapture`
    /// (`RadarUpgrade.cpp:96-124`) and `CostModifierUpgrade::onCapture`
    /// (`CostModifierUpgrade.cpp:139-162`) move the player-level effect from the old owner to the new
    /// one (a captured radar facility or tech oil refinery).
    pub(crate) fn upgrade_module_capture(&mut self, id: ObjectId, old_player: PlayerIndex, new_player: PlayerIndex) {
        if old_player == new_player {
            return;
        }
        let Some(o) = self.find_object(id) else { return };
        let disabled = o.is_disabled();
        let radar: Vec<bool> = if disabled {
            Vec::new()
        } else {
            o.template
                .upgrade_modules
                .iter()
                .filter_map(|m| match m.effect {
                    UpgradeEffect::Radar { disable_proof } if o.applied_upgrades.contains(&format!("radar:{disable_proof}")) => Some(disable_proof),
                    _ => None,
                })
                .collect()
        };
        let costs: Vec<(u64, f32)> = o
            .template
            .upgrade_modules
            .iter()
            .filter_map(|m| match m.effect {
                UpgradeEffect::CostModifier { kind_of, percentage }
                    if o.applied_upgrades.contains(&format!("costmod:{kind_of}:{percentage}")) =>
                {
                    Some((kind_of, percentage))
                }
                _ => None,
            })
            .collect();
        for disable_proof in radar {
            if let Some(p) = self.players.get_mut(&old_player) {
                p.remove_radar(disable_proof);
            }
            if let Some(p) = self.players.get_mut(&new_player) {
                p.add_radar(disable_proof);
            }
        }
        for (kind_of, percentage) in costs {
            self.remove_production_cost_change(old_player, kind_of, percentage);
            self.add_production_cost_change(new_player, kind_of, percentage);
        }
    }

    // ---- set flags ---------------------------------------------------------------------------

    /// `Object::setWeaponSetFlag` (`Object.cpp:3101`): set one `WEAPONSET_*` bit.
    pub fn set_weapon_set_flag(&mut self, id: ObjectId, flag: u32) {
        let Some(o) = self.find_object(id) else { return };
        let flags = o.weapon_set_flags | flag;
        self.replace_weapon_set_flags(id, flags);
    }

    /// `Object::clearWeaponSetFlag` (`Object.cpp:3112`): clear one `WEAPONSET_*` bit.
    pub fn clear_weapon_set_flag(&mut self, id: ObjectId, flag: u32) {
        let Some(o) = self.find_object(id) else { return };
        let flags = o.weapon_set_flags & !flag;
        self.replace_weapon_set_flags(id, flags);
    }

    /// `Object::setArmorSetFlag` (`Object.cpp:1232`): set one `ARMORSET_*` bit.
    pub fn set_armor_set_flag(&mut self, id: ObjectId, flag: u32) {
        let Some(o) = self.find_object(id) else { return };
        let flags = o.armor_set_flags | flag;
        self.replace_armor_set_flags(id, flags);
    }

    /// `Object::clearArmorSetFlag` (`Object.cpp:1238`): clear one `ARMORSET_*` bit.
    pub fn clear_armor_set_flag(&mut self, id: ObjectId, flag: u32) {
        let Some(o) = self.find_object(id) else { return };
        let flags = o.armor_set_flags & !flag;
        self.replace_armor_set_flags(id, flags);
    }

    /// Replace the object's `WEAPONSET_*` flags and re-select its weapon set: `WeaponSet::updateWeaponSet`
    /// (`WeaponSet.cpp:320`). The weapons are rebuilt only when the selected template changes
    /// (`set != m_curWeaponTemplateSet`, pointer identity), and a change releases the weapon lock
    /// (`releaseWeaponLock(LOCKED_PERMANENTLY)`).
    fn replace_weapon_set_flags(&mut self, id: ObjectId, flags: u32) {
        let Some(o) = self.objects.get(&id) else { return };
        let old_flags = o.weapon_set_flags;
        if old_flags == flags {
            return;
        }
        let template = o.template.clone();
        let old_set = select_weapon_set(&template.weapon_sets, old_flags);
        let new_set = select_weapon_set(&template.weapon_sets, flags);
        let changed = match (old_set, new_set) {
            (Some(a), Some(b)) => !std::ptr::eq(a, b),
            (None, None) => false,
            _ => true,
        };
        let now = self.frame;
        if let Some(o) = self.objects.get_mut(&id) {
            o.weapon_set_flags = flags;
            if changed {
                if let Some(set) = new_set {
                    // The rebuilt weapon set holds no lock: both kinds are released (`releaseWeaponLock(PERMANENTLY)`,
                    // which also drops a temporary status), and `updateWeaponSet` resets `m_curWeapon` to
                    // PRIMARY (`WeaponSet.cpp:330-336`). `WeaponLockSharedAcrossSets` is not modelled.
                    o.combat.rebuild_weapon_set(set, now);
                    if let Some(a) = o.combat.attack.as_mut() {
                        a.locked_slot = None;
                    }
                }
            }
        }
    }

    /// Replace the object's `ARMORSET_*` flags and re-select its armor (`ActiveBody::validateArmorAndDamageFX`
    /// picks the armor from the flags, `ActiveBody.cpp:262`). The body's armor is set only on a change.
    fn replace_armor_set_flags(&mut self, id: ObjectId, flags: u32) {
        let Some(o) = self.objects.get(&id) else { return };
        if o.armor_set_flags == flags {
            return;
        }
        let template = o.template.clone();
        let armor = select_armor_set(&template.armor_sets, flags).flatten();
        if let Some(o) = self.objects.get_mut(&id) {
            o.armor_set_flags = flags;
            if let Some(b) = o.body.as_mut() {
                b.set_armor(armor);
            }
        }
    }

    // ---- experience ------------------------------------------------------------------------

    /// The player and experience part of `Object::scoreTheKill` (Object.cpp:2959), run when a damage
    /// kill happens (`ActiveBody.cpp:672`, before `onDie`). Nothing is paid unless the victim is an
    /// enemy of the killer, on a playable side (not the neutral side), not `KINDOF_IGNORED_IN_GUI`, and
    /// not the killer's own unit. A victim under construction pays no skill points, no bounty and no
    /// experience (Player.cpp:2466 and :2531, Object.cpp:3001).
    pub(crate) fn grant_kill_experience(&mut self, victim: ObjectId, killer: Option<ObjectId>) {
        let Some(k) = killer.filter(|k| *k != ObjectId::INVALID) else { return };
        let (Some(v), Some(kobj)) = (self.find_object(victim), self.find_object(k)) else { return };
        let (victim_player, victim_uc, victim_template) = (v.player, v.under_construction, v.template.clone());
        let (killer_player, victim_level) = (kobj.player, v.experience.level);
        // `Object::scoreTheKill`: `victimController->isPlayableSide()`, `KINDOF_IGNORED_IN_GUI`.
        if Some(victim_player) == self.neutral_player() || victim_template.ignored_in_gui {
            return;
        }
        // `getRelationship(victim) == ENEMIES`, and not the killer's own player.
        if killer_player == victim_player || self.relationship(killer_player, victim_player) != crate::damage::Relationship::Enemies {
            return;
        }
        // srj sez: no experience (et al) for killing things under construction.
        if victim_uc {
            return;
        }
        let value = victim_template.experience_value[victim_level.index()];
        // `Player::addSkillPointsForKill` (Player.cpp:2525): `victim->getTemplate()->getSkillPointValue(victimLevel)`.
        let skill = victim_template.skill_point_value_at(victim_level.index());
        self.add_skill_points(killer_player, skill.max(0) as u32);
        // `Player::doBountyForKill` (Player.cpp:2458, `CashBountyPower`): the killer's owner gets
        // `ceil(cost * percent)` (the RETAIL_COMPATIBLE_CRC rounding) and the amount is `addMoneyEarned`.
        // `Player::scoreTheKill` uses `calcCostToBuild(victim->getControllingPlayer())` (:2467).
        let bounty_pct = self.players.get(&killer_player).map_or(0.0, |p| p.cash_bounty);
        let bounty = (self.calc_cost_to_build(victim_player, &victim_template) as f32 * bounty_pct).ceil() as u32;
        if bounty > 0 {
            self.deposit_cash(killer_player, bounty);
            self.score_money_earned(killer_player, bounty);
        }
        // `Object::scoreTheKill` experience: the victim's experience value, if the killer accepts points.
        self.add_experience(k, value);
    }

    /// `ExperienceTracker::addExperiencePoints`; on a level change, update the upgrade modules
    /// (veterancy condition flags).
    pub fn add_experience(&mut self, id: ObjectId, gain: i32) {
        let Some(o) = self.find_object(id) else { return };
        let required = o.template.experience_required;
        let scale = o.experience.experience_scalar;
        let sink = o.experience.sink;
        if let Some(sink) = sink {
            if self.find_object(sink).is_some() {
                self.add_experience(sink, (gain as f32 * scale) as i32);
                return;
            }
        }
        let changed = self.objects.get_mut(&id).and_then(|o| o.experience.add_experience(gain, &required, true));
        if let Some((old, new)) = changed {
            self.on_veterancy_level_changed(id, old, new);
        }
    }

    /// `ExperienceTracker::setVeterancyLevel` (explicit setting).
    pub fn set_veterancy_level(&mut self, id: ObjectId, level: crate::experience::VeterancyLevel) {
        let Some(o) = self.find_object(id) else { return };
        let required = o.template.experience_required;
        let changed = self.objects.get_mut(&id).and_then(|o| o.experience.set_veterancy_level(level, &required));
        if let Some((old, new)) = changed {
            self.on_veterancy_level_changed(id, old, new);
        }
    }

    /// The condition flags used by weapon bonuses: veterancy, `PLAYER_UPGRADE` and garrisoned.
    /// A contained unit also inherits its container's flags when the container has
    /// `WeaponBonusPassedToPassengers` (`Weapon::computeBonus`); only a `GarrisonContain` sets
    /// `GARRISONED` on its occupants (`GarrisonContain::onContaining`).
    pub fn weapon_bonus_flags(&self, id: ObjectId) -> u32 {
        let Some(o) = self.find_object(id) else { return 0 };
        let mut flags = 0u32;
        if let Some(container) = o.contained_by {
            if self.is_garrison_container(container) {
                flags |= 1 << crate::bonus::condition::GARRISONED;
            }
            if self
                .find_object(container)
                .and_then(|c| c.template.contain.as_ref())
                .is_some_and(|ct| ct.weapon_bonus_passed_to_passengers)
            {
                flags |= self.weapon_bonus_flags(container);
            }
        }
        // `WEAPONBONUSCONDITION_PLAYER_UPGRADE` comes only from `WeaponBonusUpgrade` (`WeaponBonusUpgrade.cpp:91`).
        if o.weapon_bonus_player_upgrade {
            flags |= 1 << crate::bonus::condition::PLAYER_UPGRADE;
        }
        // The other `WEAPONBONUSCONDITION_*` bits the object holds: the continuous-fire levels set by the
        // `FiringTracker`, the timed bonuses of the `TempWeaponBonusHelper` (`Object::m_weaponBonusCondition`),
        // and HORDE / NATIONALISM / FANATICISM, which only `AIUpdateInterface::evaluateMoraleBonus` sets
        // (`AIUpdate.cpp:4790-4824`); see [`GameLogic::evaluate_morale_bonus`].
        flags |= o.combat.weapon_bonus_conditions;
        let v = o.template.combat.weapon_flag_bits.veterancy(o.experience.level);
        if v & o.template.combat.weapon_flag_bits.veteran != 0 {
            flags |= 1 << crate::bonus::condition::VETERAN;
        }
        if v & o.template.combat.weapon_flag_bits.elite != 0 {
            flags |= 1 << crate::bonus::condition::ELITE;
        }
        if v & o.template.combat.weapon_flag_bits.hero != 0 {
            flags |= 1 << crate::bonus::condition::HERO;
        }
        flags
    }

    /// `AIUpdateInterface::hasNationalism` (`AIUpdate.cpp:4708-4721`): the hard-coded
    /// `Upgrade_Nationalism` (the shipped name; there is no `Upgrade_ChinaNationalism`).
    fn player_has_nationalism(&self, player: PlayerIndex) -> bool {
        self.player_has_upgrade(player, "Upgrade_Nationalism")
    }

    /// `AIUpdateInterface::hasFanaticism` (`AIUpdate.cpp:4726-4738`): the hard-coded `Upgrade_Fanaticism`.
    fn player_has_fanaticism(&self, player: PlayerIndex) -> bool {
        self.player_has_upgrade(player, "Upgrade_Fanaticism")
    }

    /// `AIUpdateInterface::evaluateMoraleBonus` (`AIUpdate.cpp:4741-4758`) with the classic
    /// `evaluateNationalismBonusClassic` action (`:4790-4824`), called by `HordeUpdate::update`
    /// (`HordeUpdate.cpp:299-301`) after every horde refresh and nowhere else outside `ALLOW_DEMORALIZE`
    /// (`setDemoralized`, which is not compiled). HORDE follows the horde state; NATIONALISM needs
    /// `AllowedNationalism` and the player's `Upgrade_Nationalism` but, in the classic action, *not* the horde
    /// state; FANATICISM needs NATIONALISM. The classic action never clears FANATICISM when nationalism stops
    /// holding (the documented retail behaviour, `AIUpdate.cpp:4793-4796`), so only NATIONALISM is cleared there.
    ///
    /// Shipped data always sets `Action = HORDE` (for example `ChinaInfantryRedguard`'s `HordeUpdate`), which is
    /// `HORDEACTION_HORDE`; `HORDEACTION_HORDE_FIXED` needs the INI `Action` field, which `zr-templates` does not
    /// carry yet (`HordeUpdate.h:63-69`).
    pub(crate) fn evaluate_morale_bonus(&mut self, id: ObjectId, in_horde: bool, allow_nationalism: bool) {
        use crate::bonus::condition;
        let Some(player) = self.find_object(id).map(|o| o.player) else { return };
        let (horde, nationalism, fanaticism) =
            (1u32 << condition::HORDE, 1u32 << condition::NATIONALISM, 1u32 << condition::FANATICISM);
        let has_nationalism = allow_nationalism && self.player_has_nationalism(player);
        let has_fanaticism = has_nationalism && self.player_has_fanaticism(player);
        let Some(o) = self.objects.get_mut(&id) else { return };
        let bits = &mut o.combat.weapon_bonus_conditions;
        if in_horde {
            *bits |= horde;
        } else {
            *bits &= !horde;
        }
        if has_nationalism {
            *bits |= nationalism;
            if has_fanaticism {
                *bits |= fanaticism;
            } else {
                *bits &= !fanaticism;
            }
        } else {
            *bits &= !nationalism;
        }
    }

    /// `Weapon::computeBonus`: the global `GameData` set first, then the weapon's own entries.
    pub fn combined_weapon_bonus(&self, id: ObjectId, wt: &crate::weapon::WeaponTemplate) -> crate::bonus::WeaponBonus {
        let flags = self.weapon_bonus_flags(id);
        let mut b = self.global_weapon_bonus.compute(flags);
        wt.weapon_bonus.append(flags, &mut b);
        b
    }

    fn player_has_upgrade(&self, player: PlayerIndex, name: &str) -> bool {
        self.player_upgrade_set(player).is_some_and(|s| s.contains(name))
    }

    /// `Object::onVeterancyLevelChanged` -> `ActiveBody::onVeterancyLevelChanged`: scale max health
    /// by `HealthBonus_<new> / HealthBonus_<old>` with `PRESERVE_RATIO` (which raises current health
    /// proportionally), then refresh the upgrade modules.
    pub(crate) fn on_veterancy_level_changed(&mut self, id: ObjectId, old: crate::experience::VeterancyLevel, new: crate::experience::VeterancyLevel) {
        if old == new {
            return;
        }
        let (old_bonus, new_bonus) = (self.health_bonus[old.index()], self.health_bonus[new.index()]);
        if old_bonus > 0.0 {
            let mult = new_bonus / old_bonus;
            if let Some(o) = self.objects.get_mut(&id) {
                let max = o.body.as_ref().map_or(0.0, |b| b.max_health());
                if let Some(b) = o.body.as_mut() {
                    b.change_max_health(max * (mult - 1.0), crate::body::MaxHealthChangeType::PreserveRatio);
                }
            }
        }
        self.update_upgrade_modules(id);
    }
}

/// Helper on `GameLogic` for the player upgrade sets (kept here so `logic.rs` stays small).
impl GameLogic {
    pub(crate) fn player_upgrade_set(&self, player: PlayerIndex) -> Option<&BTreeSet<String>> {
        self.player_upgrades.get(&player)
    }
    pub(crate) fn player_upgrade_set_mut(&mut self, player: PlayerIndex) -> &mut BTreeSet<String> {
        self.player_upgrades.entry(player).or_default()
    }

    /// `ProductionUpdate::queueUpgrade`: enqueue an upgrade at the factory for its owner. Blocked
    /// when the player already owns it or it is already queued at this factory; the cost is
    /// withdrawn up front (`calcCostToBuild`) and refunded on cancel. A player upgrade that the
    /// player is already producing at another of its factories is blocked too (`ProductionUpdate.cpp:288-290`,
    /// `hasUpgradeInProduction`).
    pub fn queue_upgrade(&mut self, player: PlayerIndex, building: ObjectId, name: &str) -> bool {
        // `UpgradeTemplate::getUpgradeType` (ProductionUpdate.cpp:266): the upgrade's `Type = OBJECT` decides the
        // branch. The sim has no upgrade-template type, so it reads the `OBJECT_UPGRADE` button that names the upgrade
        // (`command_button_upgrades`, the table the team actions use). An upgrade with no such button is a player upgrade.
        let object = self.scripts.command_button_upgrades.values().any(|b| b.object && b.upgrade.eq_ignore_ascii_case(name));
        self.queue_upgrade_typed(player, building, name, object)
    }

    /// `MSG_QUEUE_UPGRADE` (the HUD's upgrade button, a replay, the harness): `Object::canProduceUpgrade`
    /// (Object.cpp:6218, the command set holds the button) and then `queue_upgrade`. `ProductionUpdate::queueUpgrade`
    /// (:281) makes the same check, but the sim's own callers (the AI, the team actions) make theirs first.
    pub(crate) fn command_queue_upgrade(&mut self, player: PlayerIndex, building: ObjectId, name: &str) -> bool {
        let Some(template) = self.find_object(building).map(|o| o.template.name.clone()) else { return false };
        let can_produce = self.template_source.as_ref().and_then(|s| s.can_build_upgrade(&template, name)) == Some(true);
        can_produce && self.queue_upgrade(player, building, name)
    }

    /// `ProductionUpdate::queueUpgrade` (ProductionUpdate.cpp:257-321) for a player upgrade (`object == false`) or
    /// an object upgrade. A player upgrade must be affordable (`canAffordUpgrade`, :268) and not owned or in
    /// production by the player (:288-290). An object upgrade is refused when the building has it or is not
    /// affected by it (:271-274), and is charged what the player has (`Money::withdraw` clamps, Money.cpp:57). Both
    /// are refused when already queued at this factory (`isUpgradeInQueue`, :277). The `canProduceUpgrade` test (:281)
    /// needs the command set, so the command-button caller makes it (`command_button.rs`).
    pub fn queue_upgrade_typed(&mut self, player: PlayerIndex, building: ObjectId, name: &str, object: bool) -> bool {
        // `UpgradeCenter::findUpgrade` finds no template for an unknown name, so nothing is queued
        // (`Object::doCommandButton` breaks at Object.cpp:5514-5517; the caller of `queueUpgrade` has no upgrade).
        let Some(info) = self.upgrade_info.get(name).copied() else { return false };
        let ok = self.find_object(building).is_some_and(|o| o.player == player && o.production.is_some());
        if !ok {
            return false;
        }
        if object {
            if self.has_upgrade(building, name) || !self.affected_by_upgrade(building, name) {
                return false;
            }
        } else if self.player_has_upgrade(player, name) {
            return false;
        }
        let already = self
            .find_object(building)
            .and_then(|o| o.production.as_ref())
            .is_some_and(|st| st.queue.iter().any(|e| matches!(&e.kind, crate::production::ProductionKind::Upgrade(n) if n == name)));
        if already || (!object && self.upgrade_in_production(player, name)) {
            return false;
        }
        // `ProductionUpdate::queueUpgrade` (:292-296): the queue is full when `m_productionCount` (every entry,
        // units and upgrades) reaches `m_maxQueueEntries`. C++ refuses before the cost is taken.
        let full = self.find_object(building).is_some_and(|o| {
            let max = o.template.production.as_ref().map_or(-1, |p| p.max_queue_entries);
            let queued = o.production.as_ref().map_or(0, |st| st.queue.len());
            max >= 0 && queued as i32 >= max
        });
        if full {
            return false;
        }
        let Some(p) = self.players.get_mut(&player) else { return false };
        if !object && p.money < info.cost {
            return false;
        }
        p.withdraw(info.cost);
        if let Some(o) = self.objects.get_mut(&building) {
            if let Some(st) = o.production.as_mut() {
                st.queue.push(crate::production::ProductionEntry {
                    kind: crate::production::ProductionKind::Upgrade(name.to_string()),
                    // `ProductionUpdate::queueUpgrade` never sets the quantity fields: the
                    // `ProductionEntry` constructor leaves `m_productionQuantityTotal` at 0
                    // (ProductionUpdate.cpp:163); only `queueCreateUnit` sets it (:407/:414).
                    quantity_total: 0,
                    quantity_produced: 0,
                    frames: 0,
                    // The constructor also leaves `m_exitDoor` alone; the entry is allocated from the
                    // zeroed pool (`MemoryPool::allocateBlockImplementation`, GameMemory.cpp:1708), so
                    // it reads `DOOR_1` (0) until something writes it, which no upgrade path does.
                    door: Some(0),
                    object_upgrade: object,
                });
                return true;
            }
        }
        false
    }

    /// `Object::hasUpgrade` (Object.cpp:4493): the object has completed the upgrade.
    pub(crate) fn has_upgrade(&self, id: ObjectId, name: &str) -> bool {
        self.find_object(id).is_some_and(|o| o.upgrades.contains(name))
    }

    /// `Object::affectedByUpgrade` (Object.cpp:4505) with `UpgradeMux::wouldUpgrade` (UpgradeModule.cpp:124): some
    /// upgrade module of the object would activate on the key of the completed player and object upgrades plus
    /// `name`. The module must name `name` in `TriggeredBy`, the key must hold none of its `ConflictsWith` (the
    /// candidate counts as held), and it must not have executed (`m_upgradeExecuted`, UpgradeModule.cpp:127-130).
    /// The port keeps no executed flag, so the flag is the module's activation on the owned upgrades: in ANY mode a
    /// trigger is owned (the candidate itself, if owned, is one of them, so an owned candidate is executed);
    /// in the all-of mode (`RequiresAllTriggers`) every trigger is owned. Object.cpp:4505-4520 then returns FALSE for
    /// an executed module, as `Player::addUpgrade` runs the modules of every owned object (Player.cpp:3143).
    pub(crate) fn affected_by_upgrade(&self, id: ObjectId, name: &str) -> bool {
        let Some(o) = self.find_object(id) else { return false };
        let player_set = self.player_upgrade_set(o.player);
        let owned = |n: &String| o.upgrades.contains(n) || player_set.is_some_and(|s| s.contains(n));
        o.template.upgrade_modules.iter().any(|m| {
            if !m.triggered_by.iter().any(|t| t == name) || m.conflicts_with.iter().any(|c| c == name || owned(c)) {
                return false;
            }
            if m.requires_all_triggers {
                // Every trigger is held once `name` is added, and the module has not run: it runs once every
                // trigger, `name` included, is owned, so an owned candidate means it already ran.
                let candidate_owned = o.upgrades.contains(name) || player_set.is_some_and(|s| s.contains(name));
                m.triggered_by.iter().all(|t| t == name || owned(t)) && !candidate_owned
            } else {
                !m.triggered_by.iter().any(owned)
            }
        })
    }

    /// The research time for a queued upgrade (`UpgradeTemplate::calcTimeToBuild`), at least one
    /// frame so an entry always completes.
    pub(crate) fn upgrade_build_frames(&self, name: &str) -> u32 {
        self.upgrade_info.get(name).map_or(1, |i| i.build_time_frames).max(1)
    }

    /// `GameData` global weapon bonus set + `HealthBonus_<Level>` (set by the adapter).
    pub fn set_global_bonuses(&mut self, weapon_bonus: crate::bonus::WeaponBonusSet, health_bonus: [f32; 4]) {
        self.global_weapon_bonus = weapon_bonus;
        self.health_bonus = health_bonus;
    }

    /// Upgrade research cost/time by name (set by the adapter from the `Upgrade` templates).
    pub fn set_upgrade_info(&mut self, info: BTreeMap<String, UpgradeInfo>) {
        self.upgrade_info = info;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::DamageInfo;
    use crate::damage::DamageType;
    use crate::math::Coord3D;
    use crate::object::ObjectId;
    use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
    use crate::terrain::TerrainLogic;

    /// A wheeled unit with a `*Damaged` speed of 0.5 and a `MaxHealth` upgrade that adds `add` with `change`.
    fn upgradable(add: f32, change: MaxHealthChangeType) -> (GameLogic, ObjectId) {
        let mut l = LocomotorParams::from_engine_units(LocomotorKind::Wheels, 1.0, 0.0, 0.2, 0.05, 0.1, true);
        l.max_speed_damaged = 0.5;
        l.max_turn_rate_damaged = 0.05;
        l.acceleration_damaged = 0.1;
        let mut t = ObjectTemplate::new("Upgradable");
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
        t.geometry.major_radius = 5.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.locomotor = Some(l);
        t.upgrade_modules = vec![Arc::new(SimUpgradeModule {
            triggered_by: vec!["Upgrade_Health".into()],
            conflicts_with: vec![],
            removes_upgrades: vec![],
            requires_all_triggers: false,
            effect: UpgradeEffect::MaxHealth { add_max_health: add, change_type: change },
        })];
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let id = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        (g, id)
    }

    fn hurt(g: &mut GameLogic, id: ObjectId, amount: f32) {
        let info = DamageInfo { source: ObjectId::INVALID, amount, kind: DamageType::SmallArms };
        g.objects.get_mut(&id).unwrap().body.as_mut().unwrap().attempt_damage(&info);
    }

    /// `MaxHealthUpgrade` with `SAME_CURRENTHEALTH` (the default, `MaxHealthUpgrade.cpp:45`) leaves the body's
    /// stored state alone (`ActiveBody::setMaxHealth`, `ActiveBody.cpp:927`). A 40 of 100 body is DAMAGED; the
    /// upgrade makes it 40 of 500 (really damaged by the ratio), but the penalty still reads DAMAGED (full speed)
    /// until the next heal. Recomputing on the upgrade (the reverted fix) gives the penalty speed 0.5 at once.
    #[test]
    fn a_same_current_health_upgrade_keeps_the_damage_state_until_the_next_heal() {
        let (mut g, id) = upgradable(400.0, MaxHealthChangeType::SameCurrentHealth);
        hurt(&mut g, id, 60.0);
        assert_eq!(g.body_damage_state(id), Some(crate::condition::BodyDamageType::Damaged));
        g.give_upgrade(id, "Upgrade_Health");
        assert_eq!(g.find_object(id).unwrap().body.as_ref().unwrap().max_health(), 500.0);
        assert_eq!(g.body_damage_state(id), Some(crate::condition::BodyDamageType::Damaged), "the stored state is kept");
        assert_eq!(g.locomotor_now(id).unwrap().max_speed, 1.0, "the penalty still reads DAMAGED");
        // The next heal recomputes the state from the health: 41 of 500 is really damaged.
        let body = g.objects.get_mut(&id).unwrap().body.as_mut().unwrap();
        body.attempt_healing(1.0);
        assert_eq!(g.body_damage_state(id), Some(crate::condition::BodyDamageType::ReallyDamaged));
        assert_eq!(g.locomotor_now(id).unwrap().max_speed, 0.5, "the really-damaged penalty applies after the heal");
    }

    /// A building with a `GrantUpgradeCreate` module that grants the object upgrade `Upgrade_Radar`
    /// (which switches a `MaxHealth` module on), plus a second object owned by the same player.
    fn grant_world(exempt_under_construction: bool, player_upgrade: bool, under_construction: bool) -> (GameLogic, ObjectId, ObjectId) {
        let mut t = ObjectTemplate::new("CommandCenter");
        t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.upgrade_modules = vec![Arc::new(SimUpgradeModule {
            triggered_by: vec!["Upgrade_Radar".into()],
            conflicts_with: vec![],
            removes_upgrades: vec![],
            requires_all_triggers: false,
            effect: UpgradeEffect::MaxHealth { add_max_health: 100.0, change_type: MaxHealthChangeType::SameCurrentHealth },
        })];
        t.grant_upgrade_create = vec![crate::template::GrantUpgradeCreateTemplate {
            upgrade: "Upgrade_Radar".into(),
            player_upgrade,
            exempt_under_construction,
        }];
        let mut other = ObjectTemplate::new("Other");
        other.kind_of = kind_of::VEHICLE;
        other.max_health = 100.0;
        other.initial_health = 100.0;
        other.upgrade_modules = t.upgrade_modules.clone();
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        let other_id = g.create_object(&Arc::new(other), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
        let t = Arc::new(t);
        let id = if under_construction {
            g.create_object_under_construction(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0)
        } else {
            g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0)
        };
        (g, id, other_id)
    }

    fn max_health(g: &GameLogic, id: ObjectId) -> f32 {
        g.find_object(id).and_then(|o| o.body.as_ref()).map_or(0.0, |b| b.max_health())
    }

    /// `GrantUpgradeCreate::onCreate` (`GrantUpgradeCreate.cpp:82-115`): with `ExemptStatus =
    /// UNDER_CONSTRUCTION` a complete object grants the upgrade at creation; an object-type upgrade goes to
    /// the object itself (`Object::giveUpgrade`), so the other object of the player is untouched.
    #[test]
    fn grant_upgrade_create_on_create_grants_an_object_upgrade_to_the_new_object() {
        let (g, id, other) = grant_world(true, false, false);
        assert!(g.find_object(id).unwrap().upgrades.contains("Upgrade_Radar"));
        assert!((max_health(&g, id) - 200.0).abs() < 0.01, "the upgrade module ran");
        assert!((max_health(&g, other) - 100.0).abs() < 0.01, "an object upgrade is not the player's");
    }

    /// `GrantUpgradeCreate::onCreate` skips a building that still has `UNDER_CONSTRUCTION`: the grant waits for
    /// `onBuildComplete` (`GrantUpgradeCreate.cpp:88-95`, then `:117-139`).
    #[test]
    fn grant_upgrade_create_waits_for_build_complete_while_under_construction() {
        let (mut g, id, _) = grant_world(true, false, true);
        assert!(g.find_object(id).unwrap().upgrades.is_empty(), "no grant while under construction");
        assert!((max_health(&g, id) - 100.0).abs() < 0.01);
        // The dozer finishes the structure: `UnderConstruction` clears, then the create-module loop runs.
        g.objects.get_mut(&id).unwrap().under_construction = false;
        g.create_modules_on_build_complete(id);
        assert!(g.find_object(id).unwrap().upgrades.contains("Upgrade_Radar"));
        assert!((max_health(&g, id) - 200.0).abs() < 0.01, "the upgrade module ran at completion");
    }

    /// Without `ExemptStatus = UNDER_CONSTRUCTION` the module has no `onCreate` effect at all (the engine tests
    /// that one bit, GrantUpgradeCreate.cpp:88-95); only `onBuildComplete` grants. A player-type upgrade goes to
    /// the controlling player (`Player::addUpgrade`), so every object of that player takes it.
    #[test]
    fn grant_upgrade_create_without_the_exemption_only_grants_at_build_complete_to_the_player() {
        let (mut g, id, other) = grant_world(false, true, false);
        assert!(g.find_object(id).unwrap().upgrades.is_empty(), "onCreate does nothing without the exemption bit");
        g.create_modules_on_build_complete(id);
        assert!(g.player_upgrades(0).any(|u| u == "Upgrade_Radar"), "a player upgrade is the player's");
        assert!((max_health(&g, id) - 200.0).abs() < 0.01);
        assert!((max_health(&g, other) - 200.0).abs() < 0.01, "the player's upgrade reaches every owned object");
    }
}
