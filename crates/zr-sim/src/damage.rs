//! Damage and death type tables (`Core/GameEngine/Include/GameLogic/Damage.h`, Zero Hour list), and the
//! damage-triggered retaliation assist (`ActiveBody::attemptDamage`'s retaliate block).
//!
//! The order matches `DamageTypeFlags::s_bitNameList` / `TheDeathNames`, so the adapter can map
//! `zr-templates` values by index (checked by a test in `zr-game`).

use crate::body::DamageInfo;
use crate::logic::GameLogic;
use crate::math::Coord3D;
use crate::object::{status, ObjectId};
use crate::template::kind_of;

/// `DamageType`. `Healing`, `Unresistable` and the subdual types get special handling in the body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum DamageType {
    Explosion,
    Crush,
    ArmorPiercing,
    SmallArms,
    Gattling,
    Radiation,
    Flame,
    Laser,
    Sniper,
    Poison,
    Healing,
    Unresistable,
    Water,
    Deploy,
    Surrender,
    Hack,
    KillPilot,
    Penalty,
    Falling,
    Melee,
    Disarm,
    HazardCleanup,
    ParticleBeam,
    Toppling,
    InfantryMissile,
    AuroraBomb,
    LandMine,
    JetMissiles,
    StealthjetMissiles,
    MolotovCocktail,
    ComancheVulcan,
    SubdualMissile,
    SubdualVehicle,
    SubdualBuilding,
    SubdualUnresistable,
    Microwave,
    KillGarrisoned,
    Status,
}

impl DamageType {
    pub const COUNT: usize = 38;
    pub fn index(self) -> usize {
        self as usize
    }
    /// Table-order lookup; out-of-range indices clamp to `Explosion` (the weapon default).
    pub fn from_index(i: usize) -> Self {
        const ALL: [DamageType; DamageType::COUNT] = [
            DamageType::Explosion,
            DamageType::Crush,
            DamageType::ArmorPiercing,
            DamageType::SmallArms,
            DamageType::Gattling,
            DamageType::Radiation,
            DamageType::Flame,
            DamageType::Laser,
            DamageType::Sniper,
            DamageType::Poison,
            DamageType::Healing,
            DamageType::Unresistable,
            DamageType::Water,
            DamageType::Deploy,
            DamageType::Surrender,
            DamageType::Hack,
            DamageType::KillPilot,
            DamageType::Penalty,
            DamageType::Falling,
            DamageType::Melee,
            DamageType::Disarm,
            DamageType::HazardCleanup,
            DamageType::ParticleBeam,
            DamageType::Toppling,
            DamageType::InfantryMissile,
            DamageType::AuroraBomb,
            DamageType::LandMine,
            DamageType::JetMissiles,
            DamageType::StealthjetMissiles,
            DamageType::MolotovCocktail,
            DamageType::ComancheVulcan,
            DamageType::SubdualMissile,
            DamageType::SubdualVehicle,
            DamageType::SubdualBuilding,
            DamageType::SubdualUnresistable,
            DamageType::Microwave,
            DamageType::KillGarrisoned,
            DamageType::Status,
        ];
        ALL.get(i).copied().unwrap_or(DamageType::Explosion)
    }
    /// `IsSubdualDamage`.
    pub fn is_subdual(self) -> bool {
        matches!(
            self,
            DamageType::SubdualMissile | DamageType::SubdualVehicle | DamageType::SubdualBuilding | DamageType::SubdualUnresistable
        )
    }
}

/// `DeathType`. Carried to the object on death for the (not yet ported) die modules.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum DeathType {
    #[default]
    Normal,
    None,
    Crushed,
    Burned,
    Exploded,
    Poisoned,
    Toppled,
    Flooded,
    Suicided,
    Lasered,
    Detonated,
    Splatted,
    PoisonedBeta,
    Extra2,
    Extra3,
    Extra4,
    Extra5,
    Extra6,
    Extra7,
    Extra8,
    PoisonedGamma,
}

impl DeathType {
    pub const COUNT: usize = 21;
    pub fn from_index(i: usize) -> Self {
        const ALL: [DeathType; DeathType::COUNT] = [
            DeathType::Normal,
            DeathType::None,
            DeathType::Crushed,
            DeathType::Burned,
            DeathType::Exploded,
            DeathType::Poisoned,
            DeathType::Toppled,
            DeathType::Flooded,
            DeathType::Suicided,
            DeathType::Lasered,
            DeathType::Detonated,
            DeathType::Splatted,
            DeathType::PoisonedBeta,
            DeathType::Extra2,
            DeathType::Extra3,
            DeathType::Extra4,
            DeathType::Extra5,
            DeathType::Extra6,
            DeathType::Extra7,
            DeathType::Extra8,
            DeathType::PoisonedGamma,
        ];
        ALL.get(i).copied().unwrap_or(DeathType::Normal)
    }
}

/// `HUGE_DAMAGE_AMOUNT` (Damage.h): used for `KILLS_SELF` and kill damage.
pub const HUGE_DAMAGE_AMOUNT: f32 = 999_999.0;

/// `Relationship` between two objects' owners (`Object::getRelationship`, reduced to alliances).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relationship {
    Enemies,
    Neutral,
    Allies,
}

/// `AIData.ini` `MaxRetaliationDistance`: beyond this (bounding-sphere surface distance) a hit does not
/// trigger `ActiveBody`'s retaliation assist. The shipped `AIData.ini` sets 200.0; the engine's constructor
/// default is 210.0 (`AI.cpp:956`).
const MAX_RETALIATION_DISTANCE: f32 = 200.0;
/// `AIData.ini` `RetaliationFriendsRadius` (`AI.cpp:957`): how far from the hit object the allies it calls
/// on may stand, plus that object's bounding circle (`ActiveBody.cpp:701`).
const RETALIATION_FRIENDS_RADIUS: f32 = 120.0;

impl GameLogic {
    /// `ActiveBody::attemptDamage`'s retaliation block (`ActiveBody.cpp:688-725`), which runs after
    /// `doDamageFX` and the repulsor block (the port's `damage_object_with_death` runs it at its end, after
    /// the die pipeline). A human player with logical retaliation mode on gets its nearby idle allies to
    /// avenge the hit. `ActiveBody::attemptDamage` has already returned for these cases, which the caller
    /// therefore skips: an `isEffectivelyDead` object (`:357`) and `DAMAGE_HEALING` (`:385-392`).
    pub(crate) fn retaliate_after_damage(&mut self, victim: ObjectId, info: &DamageInfo) {
        // The object may have died in this call; `Object::onDie` does not remove it until the frame's
        // `processDestroyList`, so its owner and position are still readable (the C++ comment: "Retaliate,
        // even if I'm dead").
        let Some(o) = self.objects.get(&victim) else { return };
        let player = o.player;
        // `controllingPlayer->isLogicalRetaliationModeEnabled() && getPlayerType() == PLAYER_HUMAN`. The sim has no
        // player-type field; a player that runs a skirmish AI is the computer one (`Game::players`' `is_ai`),
        // everyone else is a human slot (`build.rs` uses the same test for human-only orders).
        if !self.players.get(&player).is_some_and(|p| p.logical_retaliation_mode_enabled) || self.is_ai_player(player) {
            return;
        }
        let (pos, radius) = (o.position, o.template.geometry.bounding_circle_radius());
        // `shouldRetaliateAgainstAggressor` (`ActiveBody.cpp:730-757`).
        let Some(d) = self.objects.get(&info.source).filter(|d| !d.destroyed) else { return };
        let (damager_pos, damager_player, damager_radius) = (d.position, d.player, d.template.geometry.bounding_circle_radius());
        if d.status_mask(self.frame) & status::AIRBORNE_TARGET != 0 {
            return; // "Don't retaliate against aircraft."
        }
        if self.relationship(player, damager_player) != Relationship::Enemies {
            return;
        }
        // `getDistanceSquared(obj, damager, FROM_BOUNDINGSPHERE_2D)`.
        let gap = pos.dist_2d(&damager_pos) - radius - damager_radius;
        if gap > MAX_RETALIATION_DISTANCE {
            return;
        }
        // `PartitionFilterPlayerAffiliation(controllingPlayer, ALLOW_ALLIES, true)` plus `PartitionFilterOnMap`
        // (`ActiveBody.cpp:696-702`); off-map objects are not modelled in the port.
        let reach = RETALIATION_FRIENDS_RADIUS + radius;
        let allies: Vec<ObjectId> = self
            .objects
            .values()
            .filter(|o| !o.destroyed && self.relationship(player, o.player) == Relationship::Allies)
            .filter(|o| o.position.dist_2d(&pos) <= reach)
            .map(|o| o.id)
            .collect();
        for id in allies {
            // `shouldRetaliate(them)` (`ActiveBody.cpp:761-792`). `KINDOF_CANNOT_RETALIATE` is not one of the
            // sim's 64 `kind_of` bits yet, so that one filter is not modelled.
            let Some(ally) = self.objects.get(&id).filter(|o| !o.destroyed) else { continue };
            if ally.is_kind_of(kind_of::IMMOBILE) || ally.is_kind_of(kind_of::DRONE) {
                continue;
            }
            if ally.ai_handle.is_none() || !self.ai_is_idle(ally) {
                continue; // "Any unit that isn't idle won't retaliate."
            }
            let st = ally.status_mask(self.frame);
            if st & status::STEALTHED != 0 && st & status::DETECTED == 0 {
                continue;
            }
            if st & status::USING_ABILITY != 0 {
                continue;
            }
            // "But only if we can attack it!": `getAbleToAttackSpecificObject(ATTACK_NEW_TARGET, damager,
            // CMD_FROM_AI)` is `ATTACKRESULT_POSSIBLE` / `_POSSIBLE_AFTER_MOVING`.
            if !self.is_able_to_attack(id) {
                continue;
            }
            let ally_player = ally.player;
            if ally.combat.weapons.is_empty() || self.target_info(&crate::combat::AttackTarget::Object(info.source), ally_player, false).is_none() {
                continue;
            }
            // `ActiveBody` passes the ally's own position (`them->getPosition()`, `ActiveBody.cpp:722`).
            let pos = ally.position;
            self.ai_guard_retaliate(id, info.source, pos);
        }
    }

    /// `ActiveBody::attemptDamage`'s assist call (`ActiveBody.cpp:722`):
    /// `ai->aiGuardRetaliate(damager, them->getPosition(), NO_MAX_SHOTS_LIMIT, CMD_FROM_AI)`.
    /// The order itself is `AIUpdateInterface::aiGuardRetaliate` (`AI.h:592-599`) ->
    /// `privateGuardRetaliate` (`AIUpdate.cpp:3470-3486`), ported in `crate::ai_guard_retaliate`: it
    /// enters `AI_GUARD_RETALIATE` and runs the `AIGuardRetaliateMachine` (the aggressor first, then a
    /// return to the guarded position, then the inner/outer chase), not the port's `AI_GUARD`.
    fn ai_guard_retaliate(&mut self, id: ObjectId, damager: ObjectId, pos: Coord3D) {
        self.ai_guard_retaliate_order(
            id,
            damager,
            pos,
            crate::ai_guard_retaliate::NO_MAX_SHOTS_LIMIT,
            crate::ai_states::CommandSource::Ai,
        );
    }
}
