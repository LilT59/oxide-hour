//! `Object` (GameLogic/Object.h): identity, transform, template, body and AI state.
//!
//! Update modules do not live in the object: like the original's `m_sleepyUpdates`, they are
//! owned by the scheduler ([`crate::schedule::UpdateSchedule`]) and the object only keeps their
//! handles. This lets the scheduler hold every module in one deterministic heap.

use crate::ai::AiState;
use crate::body::{BodyModule, DamageInfo};
use crate::combat::ObjectCombat;
use crate::crc::Crc;
use crate::math::Coord3D;
use crate::schedule::UpdateHandle;
use crate::template::ObjectTemplate;
use std::sync::Arc;

/// `ObjectID`: allocated monotonically by `GameLogic::allocateObjectID`, never reused.
/// `0` is `INVALID_ID`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectId(pub u32);

impl ObjectId {
    pub const INVALID: ObjectId = ObjectId(0);
}

/// Player index. The sim treats it as an opaque owner id (the `Player` subsystem is not ported).
pub type PlayerIndex = u8;

/// `Locomotor::BIGNUM` (`Locomotor.cpp:60`): the "unlimited" `m_maxLift` / `m_maxSpeed`.
pub const LOCO_BIGNUM: f32 = 99999.0;

/// `ObjectStatusTypes` as `ObjectStatusMaskType` bits: bit `i` is `OBJECT_STATUS_*` with enum value
/// `i` (`ObjectStatusTypes.h`). `zr_templates::ObjectStatus` uses the same order, so a template
/// `Mask<ObjectStatus>` converts with `bits()` alone. `OBJECT_STATUS_NONE` (value 0) has no bit.
/// The 45 statuses fit a `u64`. Pinned against the template enum by `zr-game/tests/set_flag_bits.rs`.
pub mod status {
    pub const DESTROYED: u64 = 1 << 1;
    pub const CAN_ATTACK: u64 = 1 << 2;
    pub const UNDER_CONSTRUCTION: u64 = 1 << 3;
    pub const UNSELECTABLE: u64 = 1 << 4;
    pub const NO_COLLISIONS: u64 = 1 << 5;
    pub const NO_ATTACK: u64 = 1 << 6;
    pub const AIRBORNE_TARGET: u64 = 1 << 7;
    pub const PARACHUTING: u64 = 1 << 8;
    pub const REPULSOR: u64 = 1 << 9;
    pub const HIJACKED: u64 = 1 << 10;
    pub const AFLAME: u64 = 1 << 11;
    pub const BURNED: u64 = 1 << 12;
    pub const WET: u64 = 1 << 13;
    pub const IS_FIRING_WEAPON: u64 = 1 << 14;
    pub const IS_BRAKING: u64 = 1 << 15;
    pub const STEALTHED: u64 = 1 << 16;
    pub const DETECTED: u64 = 1 << 17;
    pub const CAN_STEALTH: u64 = 1 << 18;
    pub const SOLD: u64 = 1 << 19;
    pub const UNDERGOING_REPAIR: u64 = 1 << 20;
    pub const RECONSTRUCTING: u64 = 1 << 21;
    pub const MASKED: u64 = 1 << 22;
    pub const IS_ATTACKING: u64 = 1 << 23;
    pub const USING_ABILITY: u64 = 1 << 24;
    pub const IS_AIMING_WEAPON: u64 = 1 << 25;
    pub const NO_ATTACK_FROM_AI: u64 = 1 << 26;
    pub const IGNORING_STEALTH: u64 = 1 << 27;
    pub const IS_CARBOMB: u64 = 1 << 28;
    pub const DECK_HEIGHT_OFFSET: u64 = 1 << 29;
    pub const RIDER1: u64 = 1 << 30;
    pub const RIDER2: u64 = 1 << 31;
    pub const RIDER3: u64 = 1 << 32;
    pub const RIDER4: u64 = 1 << 33;
    pub const RIDER5: u64 = 1 << 34;
    pub const RIDER6: u64 = 1 << 35;
    pub const RIDER7: u64 = 1 << 36;
    pub const RIDER8: u64 = 1 << 37;
    pub const FAERIE_FIRE: u64 = 1 << 38;
    pub const KILLING_SELF: u64 = 1 << 39;
    pub const REASSIGN_PARKING: u64 = 1 << 40;
    pub const BOOBY_TRAPPED: u64 = 1 << 41;
    pub const IMMOBILE: u64 = 1 << 42;
    pub const DISGUISED: u64 = 1 << 43;
    pub const DEPLOYED: u64 = 1 << 44;
}

/// The mutable part of a `Locomotor` instance: `m_maxLift`, `m_maxSpeed` and the flag bits the jet AI
/// toggles (`Locomotor.h`: `setMaxLift`, `setMaxSpeed`, `setUsePreciseZPos`, `setUltraAccurate`,
/// `setAllowInvalidPosition`, `setNoSlowDownAsApproachingDest`), plus the `CLIMBING` flag the climb kernel
/// keeps between frames (`Locomotor.h:435`, `Locomotor.cpp:1732-1737`). The engine builds a fresh
/// `Locomotor` on every locomotor-set change (`AIUpdateInterface::chooseLocomotorSetExplicit`), which
/// resets these; [`LocoRuntime::default`] is that fresh state (`m_flags = 0`, `Locomotor.cpp:658`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocoRuntime {
    pub max_lift: f32,
    pub max_speed: f32,
    pub precise_z: bool,
    pub ultra_accurate: bool,
    pub allow_invalid_position: bool,
    /// `Locomotor::NO_SLOW_DOWN_AS_APPROACHING_DEST` (`Locomotor.h:433-436`): a contact-weapon attack
    /// approach runs at full speed to its target (`AIUpdate.cpp:1869-1872` sets it; a locomotor-set change
    /// clears it, `:884-887`).
    pub no_slow_down_as_approaching_dest: bool,
    /// `Locomotor::CLIMBING`.
    pub climbing: bool,
}

impl Default for LocoRuntime {
    fn default() -> Self {
        LocoRuntime {
            max_lift: LOCO_BIGNUM,
            max_speed: LOCO_BIGNUM,
            precise_z: false,
            ultra_accurate: false,
            allow_invalid_position: false,
            no_slow_down_as_approaching_dest: false,
            climbing: false,
        }
    }
}

impl LocoRuntime {
    pub(crate) fn crc(&self, c: &mut Crc) {
        c.add_f32(self.max_lift);
        c.add_f32(self.max_speed);
        c.add_bool(self.precise_z);
        c.add_bool(self.ultra_accurate);
        c.add_bool(self.allow_invalid_position);
        c.add_bool(self.no_slow_down_as_approaching_dest);
        c.add_bool(self.climbing);
    }
}

/// `Object::reactToTransformChange`'s dirty rule (`Object.cpp:1817-1866`) with `isPosDifferent` /
/// `isAngleDifferent` (`:1761-1795`): the partition is only invalidated when the object moved more than 0.01
/// on an axis or turned more than 0.01 rad. Physics makes tiny changes while an object stands still, and the
/// engine deliberately keeps the partition (and so the contact list) untouched for those.
fn partition_changed(old_pos: Coord3D, new_pos: Coord3D, old_angle: f32, new_angle: f32) -> bool {
    const THRESH: f32 = 0.01;
    (old_pos.x - new_pos.x).abs() > THRESH
        || (old_pos.y - new_pos.y).abs() > THRESH
        || (old_pos.z - new_pos.z).abs() > THRESH
        || (old_angle - new_angle).abs() > THRESH
}

pub struct Object {    pub id: ObjectId,
    pub template: Arc<ObjectTemplate>,
    pub player: PlayerIndex,
    /// `Object::m_team` (`Object.cpp:196`, set by `setTeam`/`setOrRestoreTeam`, `Object.cpp:851-948`):
    /// the script team instance the object belongs to, as its unique `ScriptEngine::teams` key. Every
    /// object has one in the engine (`Object::Object` falls back to the neutral player's default team,
    /// `Object.cpp:312`); the sim only leaves it empty before the default-team prototypes exist.
    /// `Object::getControllingPlayer()` is the team's controlling player, so `player` follows the team.
    pub team: Option<String>,
    pub position: Coord3D,
    /// `Object::m_triggerInfo` and the pending check of `setTriggerAreaFlagsForChangeInPosition` (lane 184).
    pub(crate) trigger: crate::guard_area::TriggerMembership,
    /// Heading in radians about +Z (`Object::getOrientation`, `Thing::m_cachedAngle`). Written only through
    /// [`Object::set_orientation`] / [`Object::set_transform_matrix`], which keep it and `transform` in step.
    pub angle: f32,
    /// `Thing::m_transform`: the object's world matrix. Its rotation rows carry the heading the engine turns
    /// with (`rotateObjAroundLocoPivot` multiplies into them, `Locomotor.cpp:2171-2180`). Its translation is the
    /// position (`Thing::setPosition`, `Thing.cpp:184-186`).
    pub transform: crate::transform::Matrix3D,
    /// `Object::m_shroudClearingRange` (`Object.cpp:286-289`): the live shroud-clearing range, initialised from the
    /// template's `ShroudClearingRange` (or `VisionRange` when that is -1) and changed by
    /// `Object::setShroudClearingRange` — the `DynamicShroudClearingRangeUpdate` ramp
    /// (`DynamicShroudClearingRangeUpdate.cpp:283`). Read through [`crate::shroud::shroud_clearing_range`], which
    /// applies `Object::getShroudClearingRange`'s under-construction rule (`Object.cpp:5237-5245`).
    pub shroud_clearing_range: f32,
    pub body: Option<Box<dyn BodyModule>>,
    pub ai: AiState,
    /// `AIUpdateInterface::m_curLocomotorSet`: which `Locomotor = SET_x` is active.
    pub locomotor_set: crate::template::LocomotorSet,
    /// `Locomotor::setPreferredHeight` override (`ChinookMoveToBldgState` lowers the hover height
    /// for a combat drop). `None` uses the locomotor's own value.
    pub preferred_height_override: Option<f32>,
    /// `AIUpdateInterface::m_upgradedLocomotors` (`LocomotorSetUpgrade`): `SET_NORMAL` means
    /// `SET_NORMAL_UPGRADED` while set.
    pub loco_upgraded: bool,
    /// The current `Locomotor` instance's runtime flags (`setMaxLift`, `setUsePreciseZPos`, ...).
    pub loco_rt: LocoRuntime,
    /// `JetAIUpdate` runtime state (a no-op for objects without a jet AI).
    pub jet: crate::jet_ai::JetRt,
    /// `ParkingPlaceBehavior` runtime state (spaces, runways, healing), for an airfield.
    pub parking: Option<crate::jet_ai::ParkingRt>,
    /// `PhysicsBehavior` state, when the object has a physics module.
    pub physics: Option<crate::physics::Physics>,
    /// Weapons, attack order, turret and death cause (see [`crate::combat`]).
    pub combat: ObjectCombat,
    /// Periodic gameplay behavior state (auto-heal, fire, poison, stealth, lifetime).
    pub behavior: crate::behaviors::BehaviorState,
    /// Superweapon / height-die update state (`HeightDieUpdate`, `NeutronMissileUpdate`, ...).
    pub superweapon: crate::superweapon::SuperweaponState,
    /// The production queue when this object is a factory (`ProductionUpdate`).
    pub production: Option<crate::production::ProductionState>,
    /// Container holding this object (`Object::getContainedBy`), if any.
    pub contained_by: Option<ObjectId>,
    /// Units this object holds (`OpenContain::m_containList`), in insertion order.
    pub contained: Vec<ObjectId>,
    /// `OpenContain::getContainedItemsMass()` (`OpenContain.cpp:835-845`): the sum of the contained
    /// objects' [`Object::get_mass`]. Cached because the locomotor's force kernels have no `GameLogic`
    /// to walk the list; [`crate::GameLogic::refresh_contained_mass`] keeps it exact whenever the list
    /// (or a descendant container's list) changes.
    pub contained_mass: f32,
    /// `OpenContain::m_passengerAllowedToFire` (OpenContain.cpp:145): the contain module's live
    /// permission, set by `PassengersFireUpgrade` (`setPassengerAllowedToFire(TRUE)`). False keeps the
    /// template's `m_passengersAllowedToFire`.
    pub contain_passengers_allowed_to_fire: bool,
    /// `AIEnter`: the container this unit is walking to.
    pub enter_target: Option<ObjectId>,
    /// `AIExit` scheduled exit frame (0 = none).
    pub exit_at: u32,
    /// `Object::m_containedByFrame`: the frame this object entered its current container, for
    /// tunnel-network healing (`TunnelTracker::healObject`).
    pub contained_frame: u32,
    /// `OBJECT_STATUS_UNDER_CONSTRUCTION`: a dozer is still building this.
    pub under_construction: bool,
    /// `Object::getConstructionPercent` (0..100).
    pub construction_percent: f32,
    /// The builder reached the structure's start point (`DozerActionMoveToActionPosState::update`,
    /// `DozerAIUpdate.cpp:356-360`): the model conditions change from `AWAITING_CONSTRUCTION` to
    /// `PARTIALLY_CONSTRUCTED | ACTIVELY_BEING_CONSTRUCTED`.
    pub construction_arrived: bool,
    /// A dozer/worker's current build target (`DozerAIUpdate` build task).
    pub build_target: Option<ObjectId>,
    /// `Object::m_builderID`: the dozer/worker that took responsibility for this object's
    /// construction (`Object::setBuilder`), so `canResumeConstructionOf` can reject a second dozer.
    pub builder: Option<ObjectId>,
    /// `BridgeTowerBehavior::m_bridgeID`: for a bridge tower, the bridge object it belongs to.
    pub bridge: Option<ObjectId>,
    /// `SlavedUpdate`: for a slave (Stinger soldier, drone), the object it follows.
    pub master: Option<ObjectId>,
    /// `SpawnBehavior`'s members: the frame countdown, the replacement list and the template iterator.
    pub spawn: crate::modules2::SpawnRuntime,
    /// `TransportContain::m_payloadCreated`: the `InitialPayload` passengers were created (first update).
    pub payload_created: bool,
    /// A dozer/worker's current repair target (`DozerAIUpdate` repair task).
    pub repair_target: Option<ObjectId>,
    /// `DozerPrimaryIdleState::m_idleTooLongTimestamp`: frame the dozer last stopped being idle.
    pub dozer_idle_frame: u32,
    /// Supply boxes held (a truck's load, or a warehouse's remaining stock).
    pub boxes: i32,
    /// A supply truck's current dock (`SupplyTruckAIUpdate`).
    pub supply_target: Option<ObjectId>,
    /// Frames left in the truck's dock action (`Supply*ActionDelay`).
    pub supply_timer: u32,
    /// `DockUpdate` runtime state (approach reservations, active docker) for dock objects.
    pub dock_state: crate::dock::DockState,
    /// `SupplyTruckAIUpdate` runtime state (its state machine and latches).
    pub supply_ai: crate::supply::SupplyAiState,
    /// `Object::m_upgrades`: names of upgrades this object has been given.
    pub upgrades: std::collections::BTreeSet<String>,
    /// Upgrade modules already applied once (`MaxHealthUpgrade` bookkeeping).
    pub applied_upgrades: std::collections::BTreeSet<String>,
    /// `ARMORSET_*` condition flags.
    pub armor_set_flags: u32,
    /// `WEAPONSET_*` condition flags.
    pub weapon_set_flags: u32,
    /// `WEAPONBONUSCONDITION_PLAYER_UPGRADE` (`WeaponBonusUpgrade.cpp:91`): a weapon-bonus condition, not
    /// a weapon-set condition. Set by that upgrade and never cleared (`setWeaponBonusCondition`).
    pub weapon_bonus_player_upgrade: bool,
    /// `ExperienceTracker`.
    pub experience: crate::experience::ExperienceTracker,
    /// Per-object special-power cooldowns (`m_specialPowerReadyFrame`), by power name.
    pub special_power_ready: std::collections::BTreeMap<String, u32>,
    /// Per-power pause counts (`SpecialPowerModule::m_pausedCount` / `m_pausedOnFrame`), by power
    /// name. Only powers that have been paused have an entry.
    pub special_power_pause: std::collections::BTreeMap<String, crate::special_power::PowerPause>,
    /// The running `SpecialAbilityUpdate` state (`m_active`).
    pub ability: Option<crate::special_power::AbilityState>,
    /// `HackInternetAIUpdate` state (`None` until the unit is first ordered to hack).
    pub hack_internet: Option<crate::hack_internet::HackState>,
    /// `Object::m_producerID`: the object that produced/spawned this one (a factory, the OCL source,
    /// or the weapon's firer). `Object::getProducerID`; used by `UpgradeDie`.
    pub producer_id: ObjectId,
    /// `SpawnBehavior::m_spawnIDs` (`SpawnBehavior.h:210`): the spawner's live spawns, in creation order. The
    /// spawner keeps the list explicitly for the slave notifications (`onDamage`/`onDie`); `onSpawnDeath` removes a
    /// dead spawn from it, and `m_spawnCount` is this list's length.
    pub spawned: Vec<ObjectId>,
    /// `SpecialPowerCompletionDie::m_creatorID`: the object whose special power this object is the
    /// payload of (`setCreator`). `INVALID` when not tracked.
    pub sp_creator: ObjectId,
    /// `Object::m_disabledTillFrame[DISABLED_COUNT]`: the frame each disabled type expires
    /// ([`crate::disabled::NEVER`] when unset). Read through [`Object::disabled_until`].
    pub disabled_till_frame: [u32; crate::disabled::DISABLED_COUNT],
    /// `Object::m_disabledMask`: which [`crate::disabled::DisabledType`]s are active. Read through
    /// [`Object::is_disabled_by_type`] and friends.
    pub disabled_mask: u16,
    pub(crate) update_handles: Vec<UpdateHandle>,
    pub(crate) ai_handle: Option<UpdateHandle>,
    /// `OBJECT_STATUS_DESTROYED`: queued on the destroy list, invisible to queries and commands.
    pub(crate) destroyed: bool,
    /// `Object::isEffectivelyDead`: the die pipeline has run (a slow death may still be in progress).
    pub(crate) effectively_dead: bool,
    /// Status bits the sim sets directly (see [`status`]): `IS_CARBOMB`, `HIJACKED`. The statuses the sim
    /// tracks in other fields are added by [`Object::status_mask`].
    pub(crate) status_bits: u64,
    /// `Object::m_privateStatus`'s `UNDETECTED_DEFECTOR` (`Object.h:693`, `:266`): a defector whose defection is
    /// still hidden; `ObjectDefectionHelper::update` clears it when the timer runs out or the object fires/dies.
    pub undetected_defector: bool,
    /// `ModelConditionFlags`, recomputed from simulation state once per frame (see
    /// [`crate::condition`]).
    pub model_conditions: crate::condition::ModelConditionFlags,
    /// `ModelConditionUpgrade`: condition bits set by an active upgrade, OR'd into the derived
    /// conditions each frame.
    pub upgrade_conditions: u128,
    /// `RiderChangeContain` runtime state (scuttle frame, rider-table model conditions).
    pub(crate) rider: crate::rider::RiderRt,
    /// `ActiveBody::m_lastDamageInfo`: the last `DamageInfo` that hit this object, for the guard
    /// aggressor retaliate (`BodyModule::getLastDamageInfo`).
    pub last_damage_info: DamageInfo,
    /// `ActiveBody::m_lastDamageCleared`: set by `clearLastAttacker`, so `getClearableLastAttacker`
    /// returns invalid until the object is hit again.
    pub last_damage_cleared: bool,
    /// `ActiveBody::m_lastDamageTimestamp`.
    pub last_damage_timestamp: u32,
    /// Position changed since the last partition update: `PartitionData::m_dirtyStatus` needs a cell update
    /// (`makeDirty(true)`, `PartitionManager.cpp:2270`). Set through [`mark_partition_dirty`](Object::mark_partition_dirty).
    pub(crate) partition_dirty: bool,
    /// `PartitionData::makeDirty(false)`: a collision check is pending without a cell update (the contact
    /// dispatch's re-dirty, `PartitionManager.cpp:2604-2613`). Set through [`mark_collision_dirty`](Object::mark_collision_dirty).
    pub(crate) collision_dirty: bool,
    /// When the object entered the partition's dirty list (`PartitionManager::prependToDirtyModules`,
    /// `PartitionManager.h:564`); the engine walks the list from its most recent entry. 0 while not in the list.
    pub(crate) dirty_stamp: u64,
    /// `RailroadBehavior::m_hasEverBeenHitched`: a carriage already on a train (`railroad.rs`).
    pub(crate) railroad_hitched: bool,
    /// `Drawable::setDrawableHidden(m_waitingInWings || m_endOfLine)` (RailroadGuideAIUpdate.cpp:1381): a train car
    /// waiting in the wings is in the world (at its origin until the track places it) but not drawn. Client-only:
    /// the viewer skips it; the sim does not read it.
    pub(crate) drawable_hidden: bool,
}

/// The clock of the partition's dirty list (see [`Object::dirty_stamp`]). Only the order of the stamps is used. The
/// counter is shared by the whole process so that a game moved to another thread keeps its order.
static DIRTY_CLOCK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The stamp for an object entering the partition's dirty list now.
pub(crate) fn next_dirty_stamp() -> u64 {
    DIRTY_CLOCK.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

impl Object {
    pub fn is_destroyed(&self) -> bool {
        self.destroyed
    }

    /// Whether the object is in the partition's dirty list (`PartitionData::isInListDirtyModules`, `:560`).
    pub(crate) fn in_dirty_list(&self) -> bool {
        self.partition_dirty || self.collision_dirty
    }

    /// `PartitionData::makeDirty(true)` (`PartitionManager.cpp:2270-2287`): a cell update is needed. An object
    /// already in the list keeps its place (a half update only upgrades its status).
    pub(crate) fn mark_partition_dirty(&mut self) {
        if !self.in_dirty_list() {
            self.dirty_stamp = next_dirty_stamp();
        }
        self.partition_dirty = true;
    }

    /// `PartitionData::makeDirty(false)` (`PartitionManager.cpp:2270-2287`): a collision check is needed, the cells
    /// are kept.
    pub(crate) fn mark_collision_dirty(&mut self) {
        if !self.in_dirty_list() {
            self.dirty_stamp = next_dirty_stamp();
        }
        self.collision_dirty = true;
    }

    /// `PartitionData::removeFromDirtyModules` (`PartitionManager.cpp:2827`): the object leaves the dirty list.
    pub(crate) fn clear_dirty(&mut self) {
        self.partition_dirty = false;
        self.collision_dirty = false;
        self.dirty_stamp = 0;
    }

    /// `PhysicsBehavior::getMass()` (`PhysicsUpdate.cpp:1089-1096`): the module's own `m_mass` plus
    /// `contain->getContainedItemsMass()` (`OpenContain.cpp:835-845`), which the port caches in
    /// [`Object::contained_mass`]. `None` for an object without a physics module, as in the engine
    /// (where there is no physics behavior to ask).
    pub(crate) fn get_mass(&self) -> Option<f32> {
        self.template.physics.map(|t| t.mass + self.contained_mass)
    }

    /// `Object::Object`'s starting `m_shroudClearingRange` (`Object.cpp:286-289`): the template's
    /// `ShroudClearingRange`, or `VisionRange` when that is -1 ("backwards compatible, and perfectly logical
    /// default").
    pub fn template_clearing_range(&self) -> f32 {
        let r = self.template.shroud_clearing_range;
        if r < 0.0 {
            self.template.vision_range
        } else {
            r
        }
    }

    /// `Object::friend_setUndetectedDefector` (`Object.cpp:819`).
    pub fn set_undetected_defector(&mut self, undetected: bool) {
        self.undetected_defector = undetected;
    }

    /// `Object::setShroudClearingRange` (`Object.cpp:5268-5280`): store the new range. The engine only re-looks when
    /// the value actually changed; the port's shroud pass recomputes every frame, so it needs no relook call.
    pub fn set_shroud_clearing_range(&mut self, range: f32) {
        if range != self.shroud_clearing_range {
            self.shroud_clearing_range = range;
        }
    }

    /// `Drawable::isDrawableHidden` for the client: a car waiting in the wings is not drawn (see `drawable_hidden`).
    pub fn is_drawable_hidden(&self) -> bool {
        self.drawable_hidden
    }

    /// Health reached zero (`BodyModule::isDead`).
    pub fn is_dead(&self) -> bool {
        self.body.as_ref().is_some_and(|b| b.is_dead())
    }

    /// Dead or already through the die pipeline: stops acting, but may linger for a slow death.
    pub fn is_dying(&self) -> bool {
        self.effectively_dead || self.is_dead()
    }

    /// `Object::getStatusBits` at logic frame `frame`: the directly set bits plus the statuses the sim
    /// keeps in other fields (`DESTROYED`, `UNDER_CONSTRUCTION`, `AFLAME`, `STEALTHED`, `DETECTED`).
    /// `DieMuxData` and the stealth required/forbidden test read this. Statuses the sim never tracks
    /// (`IS_FIRING_WEAPON`, `SOLD`, `RIDERn`, ...) stay absent.
    pub fn status_mask(&self, frame: u32) -> u64 {
        let mut bits = self.status_bits;
        if self.destroyed {
            bits |= status::DESTROYED;
        }
        if self.under_construction {
            bits |= status::UNDER_CONSTRUCTION;
        }
        if self.behavior.aflame_until > frame {
            bits |= status::AFLAME;
        }
        if self.behavior.stealthed {
            bits |= status::STEALTHED;
        }
        if self.behavior.detected_until > frame {
            bits |= status::DETECTED;
        }
        bits
    }

    pub fn is_kind_of(&self, bits: u64) -> bool {
        self.template.is_kind_of(bits)
    }

    /// `ActiveBody::getClearableLastAttacker` (`ActiveBody.h:92`): the last attacker until
    /// `clearLastAttacker` is called.
    pub fn clearable_last_attacker(&self) -> ObjectId {
        if self.last_damage_cleared {
            ObjectId::INVALID
        } else {
            self.last_damage_info.source
        }
    }

    /// `ActiveBody::clearLastAttacker`.
    pub fn clear_last_attacker(&mut self) {
        self.last_damage_cleared = true;
    }

    /// The locomotor parameters for the object's current set.
    pub fn locomotor(&self) -> Option<crate::template::LocomotorParams> {
        let mut l = self.template.locomotor_for(self.locomotor_set)?;
        if let Some(h) = self.preferred_height_override {
            l.preferred_height = h;
        }
        // The `Locomotor` instance's runtime state (`getMaxLift` / `getMaxSpeedForCondition` clamp to
        // `m_maxLift` / `m_maxSpeed`; `getMaxTurnRate` doubles for `ULTRA_ACCURATE`).
        let rt = &self.loco_rt;
        if rt.max_lift < l.lift {
            l.lift = rt.max_lift;
        }
        if rt.max_lift < l.lift_damaged {
            l.lift_damaged = rt.max_lift;
        }
        if rt.max_speed < l.max_speed {
            l.max_speed = rt.max_speed;
        }
        if rt.max_speed < l.max_speed_damaged {
            l.max_speed_damaged = rt.max_speed;
        }
        l.precise_z = rt.precise_z;
        l.ultra_accurate = rt.ultra_accurate;
        if rt.ultra_accurate {
            const TURN_FACTOR: f32 = 2.0;
            l.max_turn_rate *= TURN_FACTOR;
            l.max_turn_rate_damaged *= TURN_FACTOR;
        }
        Some(l)
    }

    /// `Thing::setPosition` (`Thing.cpp:177-205`): the matrix translation and the cached position are both set.
    pub fn set_position(&mut self, p: Coord3D) {
        self.transform.set_translation(p);
        if p != self.position {
            if partition_changed(self.position, p, self.angle, self.angle) {
                self.mark_partition_dirty();
            }
            self.position = p;
            self.trigger.pending = true;
        }
    }

    /// `Thing::setOrientation(angle)` (`Thing.cpp:211-258`): the rotation rows are rebuilt from `Cos(angle)` and
    /// `Sin(angle)` and the translation is kept; the cached heading is `normalizeAngle(angle)`.
    pub fn set_orientation(&mut self, angle: f32) {
        let pos = self.transform.translation();
        self.transform = crate::transform::Matrix3D::from_orientation(angle, pos);
        if partition_changed(self.position, pos, self.angle, crate::math::normalize_angle(angle)) {
            self.mark_partition_dirty();
        }
        self.angle = crate::math::normalize_angle(angle);
        self.trigger.pending |= pos != self.position;
        self.position = pos;
    }

    /// `Thing::setTransformMatrix` (`Thing.cpp:263-283`): the matrix is taken as given; the position is its
    /// translation and the heading is `Get_Z_Rotation()` (no normalisation).
    pub fn set_transform_matrix(&mut self, m: crate::transform::Matrix3D) {
        self.transform = m;
        let pos = m.translation();
        if partition_changed(self.position, pos, self.angle, m.z_rotation()) {
            self.mark_partition_dirty();
        }
        self.trigger.pending |= pos != self.position;
        self.position = pos;
        self.angle = m.z_rotation();
    }

    pub fn health(&self) -> f32 {
        self.body.as_ref().map_or(0.0, |b| b.health())
    }

    pub(crate) fn crc(&self, c: &mut Crc) {
        c.add_u32(self.id.0);
        c.add_str(&self.template.name);
        c.add_u32(u32::from(self.player));
        // `Object::m_team` (`Team::getID` in the engine's object CRC note, Object.cpp:3945).
        match &self.team {
            Some(t) => {
                c.add_bool(true);
                c.add_str(t);
            }
            None => c.add_bool(false),
        }
        c.add_coord(&self.position);
        c.add_f32(self.angle);
        // `Object::m_shroudClearingRange`: hashed only when the live value differs from the template's starting
        // value, so only objects whose range a module ramps add to the CRC.
        if self.shroud_clearing_range != self.template_clearing_range() {
            c.add_f32(self.shroud_clearing_range);
        }
        c.add_bool(self.destroyed);
        c.add_bool(self.effectively_dead);
        // `Object::m_privateStatus`'s `UNDETECTED_DEFECTOR`: hashed only while set.
        if self.undetected_defector {
            c.add_bool(true);
        }
        c.add_u32(self.status_bits as u32);
        c.add_u32((self.status_bits >> 32) as u32);
        let bits = self.model_conditions.bits();
        for shift in [0, 32, 64, 96] {
            c.add_u32((bits >> shift) as u32);
        }
        c.add_u32(self.locomotor_set as u32);
        if self.loco_rt != LocoRuntime::default() {
            self.loco_rt.crc(c);
        }
        if self.template.jet.is_some() {
            self.jet.crc(c);
        }
        if let Some(p) = &self.parking {
            p.crc(c);
        }
        match self.preferred_height_override {
            Some(h) => {
                c.add_bool(true);
                c.add_f32(h);
            }
            None => c.add_bool(false),
        }
        match &self.physics {
            Some(p) => {
                c.add_bool(true);
                p.crc(c);
            }
            None => c.add_bool(false),
        }
        match &self.body {
            Some(b) => {
                c.add_bool(true);
                b.crc(c);
            }
            None => c.add_bool(false),
        }
        self.ai.crc(c);
        self.combat.crc(c);
        self.behavior.crc(c);
        match &self.production {
            Some(p) => {
                c.add_bool(true);
                p.crc(c);
            }
            None => c.add_bool(false),
        }
        c.add_u32(self.contained_by.map_or(0xffff_ffff, |x| x.0));
        c.add_u32(self.contained.len() as u32);
        for id in &self.contained {
            c.add_u32(id.0);
        }
        c.add_u32(self.enter_target.map_or(0xffff_ffff, |x| x.0));
        c.add_u32(self.exit_at);
        c.add_u32(self.contained_frame);
        c.add_bool(self.under_construction);
        c.add_f32(self.construction_percent);
        c.add_u32(self.build_target.map_or(0xffff_ffff, |x| x.0));
        // Repair target, builder and idle timestamp are hashed only while active, so objects that
        // never repair/build keep their historical contribution.
        if let Some(r) = self.repair_target {
            c.add_u32(r.0);
        }
        if let Some(b) = self.builder {
            c.add_u32(b.0);
        }
        if self.dozer_idle_frame != 0 {
            c.add_u32(self.dozer_idle_frame);
        }
        c.add_u32(self.boxes as u32);
        c.add_u32(self.supply_target.map_or(0xffff_ffff, |x| x.0));
        c.add_u32(self.supply_timer);
        self.dock_state.crc(c);
        self.supply_ai.crc(c);
        c.add_u32(self.upgrades.len() as u32);
        for u in &self.upgrades {
            c.add_str(u);
        }
        c.add_u32(self.armor_set_flags);
        c.add_u32(self.weapon_set_flags);
        c.add_u32(self.weapon_bonus_player_upgrade as u32);
        c.add_u32(self.rider.scuttled_on_frame);
        for i in 0..4 {
            c.add_u32((self.rider.conditions >> (32 * i)) as u32);
        }
        self.experience.crc(c);
        c.add_u32(self.special_power_ready.len() as u32);
        for (name, ready) in &self.special_power_ready {
            c.add_str(name);
            c.add_u32(*ready);
        }
        // Only hashed when a power is paused, so objects without one keep their historical CRC.
        if !self.special_power_pause.is_empty() {
            c.add_u32(self.special_power_pause.len() as u32);
            for (name, p) in &self.special_power_pause {
                c.add_str(name);
                c.add_u32(p.count as u32);
                c.add_u32(p.paused_on_frame);
            }
        }
        match &self.ability {
            Some(a) => {
                c.add_bool(true);
                c.add_str(&a.power);
                c.add_u32(a.packing as u32);
                c.add_u32(a.anim_frames);
                c.add_u32(a.prep_frames);
                c.add_bool(a.within_start_range);
                c.add_bool(a.facing_initiated);
                c.add_u32(a.target_obj.map_or(0xffff_ffff, |x| x.0));
            }
            None => c.add_bool(false),
        }
        // Hashed only when set, so units that never hacked keep their historical CRC.
        if let Some(h) = &self.hack_internet {
            h.crc(c);
        }
        // The derived accessors keep the historical contribution for the common no-disable case; the
        // per-type mask is only added when a type is active (see `disabled::disabled_crc`).
        c.add_u32(self.disabled_until());
        c.add_bool(self.unmanned());
        self.disabled_crc(c);
        // Producer / special-power creator are hashed only when set, so objects created by simple
        // `create_object` calls keep their historical CRC.
        if self.producer_id != ObjectId::INVALID {
            c.add_u32(self.producer_id.0);
        }
        if self.sp_creator != ObjectId::INVALID {
            c.add_u32(self.sp_creator.0);
        }
        // `RailroadBehavior::m_hasEverBeenHitched`, hashed only when set (other objects keep their CRC).
        if self.railroad_hitched {
            c.add_bool(true);
        }
        if !self.superweapon.is_default() {
            self.superweapon.crc(c);
        }
        // `ActiveBody`'s last-damage record (the guard aggressor reads it); hashed only when set so
        // objects that have never been hit keep their historical contribution.
        if self.last_damage_info.source != ObjectId::INVALID {
            c.add_u32(self.last_damage_info.source.0);
            c.add_u32(self.last_damage_timestamp);
        }
        c.add_bool(self.last_damage_cleared);
    }
}
