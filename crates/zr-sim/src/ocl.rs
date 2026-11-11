//! `ObjectCreationList` execution (GameLogic/Object/ObjectCreationList.cpp).
//!
//! `zr-sim` does not depend on the INI crate: the adapter converts `zr_templates::ObjectCreationList`
//! into [`OclTemplate`] values (object templates resolved, units already per frame / radians) and
//! attaches them to the die modules that reference them.
//!
//! Ported: the `CreateObject` / `CreateDebris` generic nugget (`GenericObjectCreationNugget::
//! reallyCreate` + `doStuffToObj`) with object creation, owner inheritance, offset, orientation,
//! the disposition RNG draw order and the disposition effects (`SEND_IT_OUT` / `SEND_IT_FLYING` /
//! `SEND_IT_UP` / `RANDOM_FORCE` forces, the spin rates, `AllowBouncing`), the
//! `FireWeapon` nugget via the existing weapon code, `ApplyRandomForceNugget` (RNG draw order and
//! physics force; the yaw/pitch/roll rates are integrated by `PhysicsBehavior::update`) and
//! `AttackNugget` (weapon lock + `aiAttackPosition` for `NumberOfShots`). `DeliverPayload` is
//! defined below.
//!
//! `CreateDebris` uses the engine's `GenericDebris` template (one object per `Count`); that template
//! has no body in the sim, so the objects are inert.

use std::sync::Arc;

use crate::combat::AttackTarget;
use crate::logic::GameLogic;
use crate::math::{atan2, cos, sin, sqrt, Coord3D, PI, TWO_PI};
use crate::object::ObjectId;
use crate::template::ObjectTemplate;
use crate::weapon::WeaponTemplate;

/// `DebrisDisposition` bits (`ObjectCreationList.cpp`), by table index.
pub mod disposition {
    pub const LIKE_EXISTING: u32 = 1 << 0;
    pub const ON_GROUND_ALIGNED: u32 = 1 << 1;
    pub const SEND_IT_FLYING: u32 = 1 << 2;
    pub const SEND_IT_UP: u32 = 1 << 3;
    pub const SEND_IT_OUT: u32 = 1 << 4;
    pub const RANDOM_FORCE: u32 = 1 << 5;
    pub const FLOATING: u32 = 1 << 6;
    pub const INHERIT_VELOCITY: u32 = 1 << 7;
    pub const WHIRLING: u32 = 1 << 8;
}

/// One `CreateObject` / `CreateDebris` nugget (`GenericObjectCreationNugget`).
#[derive(Clone, Debug, Default)]
pub struct CreateObjectTemplate {
    /// `true` for `CreateObject` (`ObjectNames`), `false` for `CreateDebris` (`ModelNames`).
    pub is_object: bool,
    /// Number of `ObjectNames` / `ModelNames` (the engine draws `pick` over this range).
    pub names_len: usize,
    /// `CreateObject`: one resolved template per name (`None` when it does not exist).
    /// `CreateDebris`: the single `GenericDebris` template (empty when it does not exist).
    pub objects: Vec<Option<Arc<ObjectTemplate>>>,
    pub count: i32,
    pub offset: (f32, f32, f32),
    pub disposition: u32,
    pub disposition_intensity: f32,
    /// Radians per frame; `-1` means "derive from `disposition_intensity`".
    pub spin_rate: f32,
    pub yaw_rate: f32,
    pub roll_rate: f32,
    pub pitch_rate: f32,
    /// Radians.
    pub min_force_pitch: f32,
    pub max_force_pitch: f32,
    pub min_force_magnitude: f32,
    pub max_force_magnitude: f32,
    pub orient_in_force_direction: bool,
    pub spread_formation: bool,
    pub min_distance_a_formation: f32,
    pub min_distance_b_formation: f32,
    pub max_distance_formation: f32,
    /// Fractions (`MinHealth` / `MaxHealth`).
    pub min_health: f32,
    pub max_health: f32,
    pub requires_live_player: bool,
}

/// One nugget of an OCL the sim executes.
#[derive(Clone, Debug)]
pub enum OclNugget {
    CreateObject(CreateObjectTemplate),
    CreateDebris(CreateObjectTemplate),
    FireWeapon(Arc<WeaponTemplate>),
    /// `DeliverPayload`: a transport flies in and drops its payload (`DeliverPayloadAIUpdate`).
    DeliverPayload(Arc<DeliverPayloadTemplate>),
    /// `ApplyRandomForce`: a physics force plus yaw/roll/pitch rates (`ApplyRandomForceNugget`).
    ApplyRandomForce(ApplyRandomForceTemplate),
    /// `Attack`: the source attacks the target position (`AttackNugget`).
    Attack(AttackTemplate),
    /// Anything else: not simulated yet.
    Unsupported,
}

/// `ApplyRandomForceNugget` (already per frame / radians).
#[derive(Clone, Copy, Debug, Default)]
pub struct ApplyRandomForceTemplate {
    /// `SpinRate`, radians per frame.
    pub spin_rate: f32,
    pub min_force_magnitude: f32,
    pub max_force_magnitude: f32,
    /// `MinForcePitch` / `MaxForcePitch`, radians.
    pub min_force_pitch: f32,
    pub max_force_pitch: f32,
}

/// `AttackNugget` (the delivery decal is client-only and not simulated).
#[derive(Clone, Copy, Debug, Default)]
pub struct AttackTemplate {
    /// `NumberOfShots`; `< 0` means no limit.
    pub number_of_shots: i32,
    /// `WeaponSlot` index (`PRIMARY_WEAPON` = 0).
    pub weapon_slot: usize,
}

/// `DeliverPayloadData` + `DeliverPayloadNugget` (sim copy). The transport is a real object flown
/// by its air locomotor to the target; the payload is dropped near the target.
#[derive(Clone, Debug)]
pub struct DeliverPayloadTemplate {
    pub transport: Option<Arc<ObjectTemplate>>,
    /// `(template, count)` per `Payload`.
    pub payload: Vec<(Arc<ObjectTemplate>, u32)>,
    pub delivery_distance: f32,
    pub drop_delay: u32,
    pub drop_offset: (f32, f32, f32),
    pub drop_variance: (f32, f32, f32),
    pub is_parachute_directly: bool,
    pub self_destruct_object: bool,
    /// `DeliverPayloadData::FireWeapon`: the transport strafes the target (A-10).
    pub fire_weapon: bool,
    pub strafing_weapon_slot: u32,
    /// `StrafeLength`: how far from the target the transport fires during the pass.
    pub strafe_length: f32,
    /// `DelayDeliveryMax` (frames): the transport is disabled for `GameLogicRandomValue(0, max)` frames after it is
    /// created (ObjectCreationList.cpp:388-390, the draw after the transport's placement :377-387).
    pub delay_delivery_frames_max: u32,
}

/// `OCLSpecialPower::CreateLocation` (OCLSpecialPower.cpp:50-58, :157-244): where the OCL's primary point is, which the
/// `DeliverPayload` transport starts from. The default is `CREATE_AT_EDGE_NEAR_SOURCE` (OCLSpecialPower.cpp:72).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CreateLocation {
    #[default]
    EdgeNearSource,
    EdgeNearTarget,
    EdgeFarthestFromTarget,
    AtLocation,
    UseOwnerObject,
    AboveLocation,
}

/// `DeliverPayloadData::m_distToTarget` slop: the transport starts `DeliveryDistance * SLOP` before the primary point
/// (ObjectCreationList.cpp:348-352).
const DELIVERY_SLOP: f32 = 1.5;

/// `CREATE_ABOVE_LOCATION_HEIGHT` (OCLSpecialPower.cpp:50).
const CREATE_ABOVE_LOCATION_HEIGHT: f32 = 300.0;

/// One in-flight delivery (`DeliverPayloadAIUpdate`), advanced by [`GameLogic::update_deliveries`].
#[derive(Clone, Debug)]
pub(crate) struct DeliveryState {
    /// `None` for a payload-only drop (`transport = None`).
    pub transport: Option<ObjectId>,
    pub target: Coord3D,
    /// `DeliveryDistance` (`getAllowedDistanceToTarget`, DeliverPayloadAIUpdate.h:322): the transport is close enough to
    /// drop once its 2D distance to the target is inside it (`isCloseEnoughToTarget`, DeliverPayloadAIUpdate.cpp:357-375).
    pub delivery_distance: f32,
    /// `DropDelay`, frames between one item's drop and the next (`DeliveringState::update`, DeliverPayloadAIUpdate.cpp:687-695).
    pub drop_delay: u32,
    /// `DeliveringState` is the current state (`DeliverPayloadAIUpdate.cpp:659-674`): the transport is close enough and
    /// the payload drops item by item. False while the approach flies (`ApproachState`, :586-607).
    pub delivering: bool,
    /// `m_dropDelayLeft`: frames until the next item drops.
    pub drop_delay_left: u32,
    /// The payload is out (`DeliveringState` returned SUCCESS, DeliverPayloadAIUpdate.cpp:700-710). Payload-only drops set it at once.
    pub drop_done: bool,
    /// The exit move has been ordered (`HeadOffMapState::onEnter`, DeliverPayloadAIUpdate.cpp:1125-1168). Once set the
    /// transport refuses AI commands (`isAllowedToRespondToAiCommands`, :145-155).
    pub exit_ordered: bool,
    /// Whether the OCL carried any payload (the transport strafes an empty payload, see `strafe_target`).
    pub has_payload: bool,
    /// The payload still to drop, one item per drop, in the transport's contain order (`items->front()`, :716).
    pub items: std::collections::VecDeque<Arc<ObjectTemplate>>,
    pub drop_offset: (f32, f32, f32),
    pub drop_variance: (f32, f32, f32),
    pub is_parachute_directly: bool,
    pub fire_weapon: bool,
    pub strafing_weapon_slot: u32,
    pub strafe_length: f32,
    pub owner: crate::object::PlayerIndex,
    /// Bombs/charges to detonate at their fall-out frame (destroy triggers the die OCL).
    pub bombs: Vec<(ObjectId, u32)>,
    /// Paratroopers still descending, with their fall speed per frame.
    pub falling: Vec<(ObjectId, f32)>,
}

/// A converted `ObjectCreationList`.
#[derive(Clone, Debug)]
pub struct OclTemplate {
    pub name: String,
    pub nuggets: Vec<OclNugget>,
}

impl GameLogic {
    /// `ObjectCreationList::createInternal`: run every nugget in order.
    pub(crate) fn run_ocl(&mut self, ocl: &Arc<OclTemplate>, source: ObjectId) {
        self.run_ocl_at(ocl, source, None, false);
    }

    /// As [`run_ocl`](Self::run_ocl) but with the special power's target. `target_pos` is used by
    /// `DeliverPayload` and by `CreateObject` when `create_at_target` (`CreateLocation`).
    pub(crate) fn run_ocl_at(
        &mut self,
        ocl: &Arc<OclTemplate>,
        source: ObjectId,
        target_pos: Option<Coord3D>,
        create_at_target: bool,
    ) {
        self.run_ocl_at_angle(ocl, source, target_pos, create_at_target, None, None);
    }

    /// As [`run_ocl_at`](Self::run_ocl_at) with the special power's build `angle` (`None` =
    /// `INVALID_ANGLE`). `GenericObjectCreationNugget::create` passes it to `reallyCreate`, where
    /// only a `LIKE_EXISTING` object keeps it (`ObjectCreationList.cpp:1038-1043`); the other
    /// nuggets ignore it (`AttackNugget`, `FireWeaponNugget`, `DeliverPayloadNugget`).
    /// `create_location` is the special power's `CreateLocation` (`OCLSpecialPower`), and `None` for every other
    /// caller: an object-based OCL's primary point is the source object's position (`ObjectCreationList.cpp:104-106`).
    pub(crate) fn run_ocl_at_angle(
        &mut self,
        ocl: &Arc<OclTemplate>,
        source: ObjectId,
        target_pos: Option<Coord3D>,
        create_at_target: bool,
        angle: Option<f32>,
        create_location: Option<CreateLocation>,
    ) {
        for nugget in &ocl.nuggets {
            match nugget {
                OclNugget::CreateObject(t) | OclNugget::CreateDebris(t) => {
                    let base = if create_at_target { target_pos } else { None };
                    self.run_create_objects(t, source, base, angle);
                }
                OclNugget::FireWeapon(w) => match target_pos {
                    Some(pos) => self.fire_temp_weapon_at(source, w.clone(), pos),
                    None => self.fire_temp_weapon(source, w.clone()),
                },
                OclNugget::DeliverPayload(d) => self.run_deliver_payload(d.clone(), source, target_pos, create_location),
                // `ApplyRandomForceNugget::create` only works on the primary object (it ignores the
                // secondary / location). `run_ocl` supplies the source object.
                OclNugget::ApplyRandomForce(t) => self.run_apply_random_force(source, t),
                // `AttackNugget::create` needs a secondary (target) position.
                OclNugget::Attack(t) => {
                    if let Some(pos) = target_pos {
                        self.ocl_attack_position(source, pos, t.number_of_shots, t.weapon_slot);
                    }
                }
                OclNugget::Unsupported => {}
            }
        }
    }

    /// `ApplyRandomForceNugget::create` (`ObjectCreationList.cpp:622`): `calcRandomForce` draws
    /// angle, pitch and magnitude in that order, applies the force, then yaw/roll/pitch rates are
    /// drawn (each in `[-spinRate, spinRate]`) and set.
    fn run_apply_random_force(&mut self, source: ObjectId, t: &ApplyRandomForceTemplate) {
        let Some(has_physics) = self.objects.get(&source).map(|o| o.physics.is_some() && o.template.physics.is_some()) else {
            return;
        };
        if !has_physics {
            return;
        }
        // `calcRandomForce`: `angle`, `pitch`, `mag` in the original's order.
        let angle = self.rng.value_real(0.0, TWO_PI);
        let pitch = self.rng.value_real(t.min_force_pitch, t.max_force_pitch);
        let mag = self.rng.value_real(t.min_force_magnitude, t.max_force_magnitude);
        let (fx, fy, fz) = calc_random_force(angle, pitch, mag);
        self.apply_force(source, Coord3D::new(fx, fy, fz));
        let yaw = self.rng.value_real(-t.spin_rate, t.spin_rate);
        let roll = self.rng.value_real(-t.spin_rate, t.spin_rate);
        let pitch_rate = self.rng.value_real(-t.spin_rate, t.spin_rate);
        self.set_angular_rates(source, yaw, roll, pitch_rate);
    }

    /// `DeliverPayloadAIUpdate`: spawn the transport, fly it through its air locomotor to the
    /// target and schedule the drop; a `transport = None` delivery drops immediately.
    fn run_deliver_payload(
        &mut self,
        d: Arc<DeliverPayloadTemplate>,
        source: ObjectId,
        target_pos: Option<Coord3D>,
        create_location: Option<CreateLocation>,
    ) {
        let Some(src) = self.objects.get(&source) else { return };
        let (target, owner, now, source_pos) = (target_pos.unwrap_or(src.position), src.player, self.frame, src.position);
        let items: std::collections::VecDeque<Arc<ObjectTemplate>> =
            d.payload.iter().flat_map(|(t, n)| std::iter::repeat_n(t.clone(), *n as usize)).collect();
        let Some(transport_t) = d.transport.clone() else {
            // No transport: drop at the target now.
            let mut state = DeliveryState {
                transport: None,
                target,
                delivery_distance: d.delivery_distance,
                drop_delay: d.drop_delay,
                delivering: false,
                drop_delay_left: 0,
                drop_done: false,
                exit_ordered: false,
                has_payload: !d.payload.is_empty(),
                items,
                drop_offset: d.drop_offset,
                drop_variance: d.drop_variance,
                is_parachute_directly: d.is_parachute_directly,
                fire_weapon: d.fire_weapon,
                strafing_weapon_slot: d.strafing_weapon_slot,
                strafe_length: d.strafe_length,
                owner,
                bombs: Vec::new(),
                falling: Vec::new(),
            };
            self.drop_all_items(&mut state);
            return;
        };
        // `DeliverPayloadNugget::create` (ObjectCreationList.cpp:331-365): the primary point is the OCL's creation point.
        // `OCLSpecialPower::doSpecialPowerAtLocation` (OCLSpecialPower.cpp:190-244) supplies it through `CreateLocation`;
        // every other OCL uses the object's position (`ObjectCreationList.cpp:104-106`, `doSpecialPower` :237-244). The
        // transport faces the approach (`orient`, from the primary to the target, before the back-off) and starts
        // `DeliveryDistance * 1.5` before the primary point along it (`startPos -= Cos(orient) * dist * SLOP`, :348-352).
        let primary = match create_location {
            Some(loc) => self.deliver_primary_point(loc, source_pos, target),
            None => source_pos,
        };
        let orient = atan2(target.y - primary.y, target.x - primary.x);
        let mut start = primary;
        if d.delivery_distance > 0.0 {
            start.x -= cos(orient) * d.delivery_distance * DELIVERY_SLOP;
            start.y -= sin(orient) * d.delivery_distance * DELIVERY_SLOP;
        }
        let transport = self.create_object(&transport_t, start, orient, owner);
        self.set_ocl_producer(transport, source);
        // `DelayDeliveryMax` (ObjectCreationList.cpp:388-390): the transport is disabled for `GameLogicRandomValue(0, max)`
        // frames, drawn only when the value is positive.
        if d.delay_delivery_frames_max > 0 {
            let delay = self.rng.value(0, d.delay_delivery_frames_max as i32) as u32;
            self.set_disabled_until(transport, crate::disabled::DisabledType::Default, now + delay);
        }
        // `ApproachState::onEnter` (DeliverPayloadAIUpdate.cpp:586): `aiMoveToPosition(getMoveToPos(), CMD_FROM_AI)`, where
        // `moveToPos` is the OCL's secondary point, the target (ObjectCreationList.cpp:336, :431).
        self.ai_move_to_position_cmd(transport, target, crate::ai_states::CommandSource::Ai);
        self.wake_up_now(transport);
        self.deliveries.push(DeliveryState {
            transport: Some(transport),
            target,
            delivery_distance: d.delivery_distance,
            drop_delay: d.drop_delay,
            delivering: false,
            drop_delay_left: 0,
            drop_done: false,
            exit_ordered: false,
            has_payload: !d.payload.is_empty(),
            items,
            drop_offset: d.drop_offset,
            drop_variance: d.drop_variance,
            is_parachute_directly: d.is_parachute_directly,
            fire_weapon: d.fire_weapon,
            strafing_weapon_slot: d.strafing_weapon_slot,
            strafe_length: d.strafe_length,
            owner,
            bombs: Vec::new(),
            falling: Vec::new(),
        });
    }

    /// The OCL's primary point for a `CreateLocation` (`OCLSpecialPower::doSpecialPowerAtLocation`, OCLSpecialPower.cpp:
    /// 190-244). `UseOwnerObject` (the owner object is the transport, :212) is not ported: the transport is still created.
    fn deliver_primary_point(&self, loc: CreateLocation, source_pos: Coord3D, target: Coord3D) -> Coord3D {
        match loc {
            CreateLocation::EdgeNearSource => self.find_closest_edge(&source_pos),
            CreateLocation::EdgeNearTarget => self.find_closest_edge(&target),
            CreateLocation::EdgeFarthestFromTarget => {
                let mut p = self.find_farthest_edge(&target);
                p.z += CREATE_ABOVE_LOCATION_HEIGHT;
                p
            }
            CreateLocation::AtLocation | CreateLocation::UseOwnerObject => target,
            CreateLocation::AboveLocation => Coord3D::new(target.x, target.y, target.z + CREATE_ABOVE_LOCATION_HEIGHT),
        }
    }

    /// `DeliverPayloadAIUpdate::isCloseEnoughToTarget` (DeliverPayloadAIUpdate.cpp:357-375), without the pre-open allowance
    /// for an inbound transport (`PreOpenDistance` is not parsed): the 2D distance to the target is inside `DeliveryDistance`.
    fn delivery_close_enough(&self, transport: ObjectId, target: Coord3D, delivery_distance: f32) -> bool {
        let Some(o) = self.objects.get(&transport) else { return false };
        let (dx, dy) = (o.position.x - target.x, o.position.y - target.y);
        delivery_distance * delivery_distance > dx * dx + dy * dy
    }

    /// `ExitState::onEnter` (DeliverPayloadAIUpdate.cpp:1142-1158): once the payload is out, the transport keeps going
    /// straight on for `HUGE_DIST` (1.2 times the map diagonal) until it leaves the map. Its `AllowInvalidPosition` flag
    /// is not modelled.
    fn deliver_exit(&mut self, transport: ObjectId) {
        let Some((pos, angle)) = self.objects.get(&transport).map(|o| (o.position, o.angle)) else { return };
        let (lo, hi) = self.terrain.world_extent();
        let huge = 1.2 * sqrt((hi.x - lo.x) * (hi.x - lo.x) + (hi.y - lo.y) * (hi.y - lo.y));
        let exit = Coord3D::new(pos.x + cos(angle) * huge, pos.y + sin(angle) * huge, pos.z);
        if let Some(o) = self.objects.get_mut(&transport) {
            // `setUltraAccurate(true)` (DeliverPayloadAIUpdate.cpp:1155), so the AI does not adjust the exit point.
            o.loco_rt.ultra_accurate = true;
        }
        self.ai_move_to_position_cmd(transport, exit, crate::ai_states::CommandSource::Ai);
    }

    /// Advance every in-flight delivery: fly the transport, drop the payload, detonate bombs and
    /// land paratroopers, then despawn the transport.
    pub(crate) fn update_deliveries(&mut self) {
        let now = self.frame;
        let list = std::mem::take(&mut self.deliveries);
        let mut keep = Vec::with_capacity(list.len());
        for mut d in list {
            // A destroyed transport takes its unloaded payload with it (the contents of a dead container), but the
            // paratroopers and bombs already out are independent objects and finish their own fall and fuse.
            if d.transport.is_some_and(|t| !self.objects.get(&t).is_some_and(|o| !o.destroyed)) {
                d.transport = None;
                d.items.clear();
            }
            if let Some(t) = d.transport {
                self.update_delivery_transport(&mut d, t);
            }
            // Detonate bombs whose fall time has elapsed (their die OCL makes the explosion).
            let bombs = std::mem::take(&mut d.bombs);
            for (bomb, when) in bombs {
                if now >= when {
                    // Damage to zero so the die pipeline (`CreateObjectDie`) runs the explosion.
                    let max_before = self.objects.keys().next_back().copied().unwrap_or(ObjectId::INVALID);
                    self.damage_object_with_death(
                        bomb,
                        crate::body::DamageInfo {
                            source: ObjectId::INVALID,
                            amount: crate::damage::HUGE_DAMAGE_AMOUNT,
                            kind: crate::damage::DamageType::Unresistable,
                        },
                        crate::damage::DeathType::Normal,
                    );
                    // Explosion effects (e.g. `DaisyCutterGas`) damage through their
                    // `SlowDeathBehavior` destruction weapon; start their slow death.
                    let new_ids: Vec<ObjectId> = self.objects.keys().copied().filter(|id| *id > max_before).collect();
                    for id in new_ids {
                        let detonates = self.objects.get(&id).is_some_and(|o| {
                            o.template.die.iter().any(|m| {
                                matches!(&m.kind, crate::die::DieTemplate::SlowDeath(sd) if !sd.weapons[2].is_empty())
                            })
                        });
                        if detonates {
                            self.damage_object_with_death(
                                id,
                                crate::body::DamageInfo {
                                    source: ObjectId::INVALID,
                                    amount: crate::damage::HUGE_DAMAGE_AMOUNT,
                                    kind: crate::damage::DamageType::Unresistable,
                                },
                                crate::damage::DeathType::Normal,
                            );
                        }
                    }
                } else {
                    d.bombs.push((bomb, when));
                }
            }
            // Paratroopers descend at their fall speed until they reach the ground.
            let falling = std::mem::take(&mut d.falling);
            for (unit, speed) in falling {
                if let Some(o) = self.objects.get_mut(&unit) {
                    let ground = self.terrain.ground_height(o.position.x, o.position.y);
                    let z = o.position.z - speed;
                    if z <= ground {
                        let mut p = o.position;
                        p.z = ground;
                        o.set_position(p);
                        // Landing: the rider leaves its chute (`ParachuteContain::onRemoving`, `ParachuteContain.cpp:473`).
                        o.status_bits &= !crate::object::status::PARACHUTING;
                    } else {
                        let mut p = o.position;
                        p.z = z;
                        o.set_position(p);
                        d.falling.push((unit, speed));
                    }
                }
            }
            // Strafing run: fire while passing the target (A-10).
            self.strafe_target(&d);
            // `HeadOffMapState::update` succeeds once `isOffMap` holds (DeliverPayloadAIUpdate.cpp:1177-1181), and
            // `CleanUpState::onEnter` destroys the transport (:1219-1229). There is no timer. A payload-only drop despawns
            // with its drop.
            if let Some(t) = d.transport {
                if d.exit_ordered && self.is_off_map(t) {
                    self.destroy_object(t);
                    d.transport = None;
                    d.items.clear();
                }
            }
            // The record lives on while its paratroopers fall or its bombs wait to detonate.
            if d.transport.is_none() && d.falling.is_empty() && d.bombs.is_empty() {
                continue;
            }
            keep.push(d);
        }
        keep.append(&mut self.deliveries);
        self.deliveries = keep;
    }

    /// `DeliverPayloadAIUpdate::isAllowedToRespondToAiCommands` (DeliverPayloadAIUpdate.cpp:145-155): once the exit is
    /// ordered (`m_acceptingCommands` cleared by `HeadOffMapState::onEnter`, :1168) the transport refuses AI commands.
    pub(crate) fn delivery_refuses_commands(&self, id: ObjectId) -> bool {
        self.deliveries.iter().any(|d| d.transport == Some(id) && d.exit_ordered)
    }

    /// `isOffMap` (DeliverPayloadAIUpdate.cpp:381-389): the position is outside the map extent including its border
    /// (`TerrainLogic::getExtentIncludingBorder`, W3DTerrainLogic.cpp:231-242), with no Z test.
    fn is_off_map(&self, id: ObjectId) -> bool {
        let Some(p) = self.objects.get(&id).map(|o| o.position) else { return true };
        let (lo, hi) = self.terrain.extent_including_border();
        p.x < lo.x || p.x > hi.x || p.y < lo.y || p.y > hi.y
    }

    /// One step of a transport's `DeliverPayloadStateMachine` (DeliverPayloadAIUpdate.cpp:516-520). APPROACH flies to the
    /// target until it is close enough (`ApproachState::update`, :592-607). DELIVERING then drops the items one by one
    /// (`DeliveringState::update`, :650-720). When the payload is out the exit is ordered in that same update
    /// (HEAD_OFF_MAP, :1125-1168), and a failed close-enough test during DELIVERING goes back to APPROACH
    /// (CONSIDER_NEW_APPROACH, :697-698, :517-518).
    fn update_delivery_transport(&mut self, d: &mut DeliveryState, t: ObjectId) {
        if d.exit_ordered {
            return;
        }
        if !d.delivering {
            if self.delivery_close_enough(t, d.target, d.delivery_distance) {
                // `DeliveringState::onEnter` (:659-674): `m_dropDelayLeft = getDoorDelay()`. `DoorDelay` is a key of the
                // transport's own AI module (DeliverPayloadAIUpdate.h:221), which the template parser does not read yet, so
                // it is 0 here (the first item drops on the next update).
                d.delivering = true;
                d.drop_delay_left = 0;
            }
            return;
        }
        if d.drop_delay_left > 0 {
            d.drop_delay_left -= 1;
            return;
        }
        // `DeliveringState::update` past its countdown (:666-698): the transport is free to exit, the next countdown is
        // `DropDelay` (:687-695), and a transport that has left the drop area goes back to its approach.
        d.drop_delay_left = d.drop_delay;
        if !self.delivery_close_enough(t, d.target, d.delivery_distance) {
            d.delivering = false;
            self.ai_move_to_position_cmd(t, d.target, crate::ai_states::CommandSource::Ai);
            return;
        }
        match d.items.pop_front() {
            Some(item) => self.drop_item(d, &item),
            None => {
                // No payload left: SUCCESS, so HEAD_OFF_MAP (:700-710, :516-520).
                d.drop_done = true;
                d.exit_ordered = true;
                self.deliver_exit(t);
            }
        }
    }

    /// Drops every remaining item at once (the payload-only path, which has no transport to fly).
    fn drop_all_items(&mut self, d: &mut DeliveryState) {
        while let Some(item) = d.items.pop_front() {
            self.drop_item(d, &item);
        }
        d.drop_done = true;
    }

    /// `DeliveringState::update`'s item drop (DeliverPayloadAIUpdate.cpp:716-745): the variance is drawn on each axis
    /// that has one (x, y, z in that order, :719-726), then the drop offset is added.
    fn drop_item(&mut self, d: &mut DeliveryState, tmpl: &Arc<ObjectTemplate>) {
        let now = self.frame;
        let vx = if d.drop_variance.0 > 0.0 { self.rng.value_real(-d.drop_variance.0, d.drop_variance.0) } else { 0.0 };
        let vy = if d.drop_variance.1 > 0.0 { self.rng.value_real(-d.drop_variance.1, d.drop_variance.1) } else { 0.0 };
        if d.drop_variance.2 > 0.0 {
            // The drop's Z variance is drawn for the stream; the item's height is set by the ground below.
            let _ = self.rng.value_real(-d.drop_variance.2, d.drop_variance.2);
        }
        let x = d.target.x + d.drop_offset.0 + vx;
        let y = d.target.y + d.drop_offset.1 + vy;
        let ground = self.terrain.ground_height(x, y);
        let z = if d.is_parachute_directly { d.target.z + 120.0 } else { ground };
        let id = self.create_object(tmpl, Coord3D::new(x, y, z), 0.0, d.owner);
        if d.is_parachute_directly && tmpl.locomotor.is_some() {
            // Keep the unit airborne and let it fall (parachute). A rider under a chute is
            // `OBJECT_STATUS_PARACHUTING` (`ParachuteContain::onContaining`, `ParachuteContain.cpp:454`).
            if let Some(o) = self.objects.get_mut(&id) {
                o.set_position(Coord3D::new(x, y, z));
                o.status_bits |= crate::object::status::PARACHUTING;
            }
            d.falling.push((id, 2.0));
        } else if tmpl.locomotor.is_none() && tmpl.max_health > 0.0 {
            // A bomb/charge: destroy it shortly so its die OCL detonates.
            d.bombs.push((id, now + 30));
        }
    }

    /// The transport fires its weapon every frame it is within `StrafeLength` of the target
    /// (`DeliverPayloadAIUpdate`'s strafing run, e.g. the A-10).
    fn strafe_target(&mut self, d: &DeliveryState) {
        let strafe = d.fire_weapon || !d.has_payload;
        let Some(t) = d.transport else { return };
        if !strafe {
            return;
        }
        let range = if d.strafe_length > 0.0 { d.strafe_length } else { 150.0 };
        let in_range = self.objects.get(&t).is_some_and(|o| o.position.dist_2d(&d.target) <= range);
        if !in_range {
            return;
        }
        let slot = d.strafing_weapon_slot as usize;
        // `Object::fireCurrentWeapon` fires only a `READY_TO_FIRE` weapon (Object.cpp:1505; the position form is
        // `:1528`), so the strafing run shoots at the weapon's own cadence, not every frame it is in range.
        let ready = self.objects.get(&t).and_then(|o| o.combat.weapons.weapon(slot)).is_some_and(|w| w.status(self.frame) == crate::weapon::WeaponStatus::ReadyToFire);
        if !ready {
            return;
        }
        // `DeliverPayloadAIUpdate.cpp:224-226` fires at the computed strafe *point* (`fireCurrentWeapon(&strafePoint)`),
        // a ground shot with no victim object; the port fires at the payload target position.
        let attack = AttackTarget::Ground(d.target);
        // Lock the strafing slot temporarily, then `fireCurrentWeapon(pos)`, which releases the temporary lock on a
        // reloaded shot (`Object.cpp:1519-1534`).
        self.set_weapon_lock(t, slot, crate::weapon::WeaponLockType::Temporarily);
        if self.fire_weapon(t, slot, attack) {
            self.release_weapon_lock(t, crate::weapon::WeaponLockType::Temporarily);
        }
    }

    /// `GenericObjectCreationNugget::reallyCreate` + `doStuffToObj`, reduced to object creation,
    /// placement and the original's RNG call order.
    fn run_create_objects(&mut self, t: &CreateObjectTemplate, source: ObjectId, base: Option<Coord3D>, build_angle: Option<f32>) {
        if t.names_len == 0 || t.count <= 0 {
            return;
        }
        let Some(src) = self.objects.get(&source) else { return };
        let (src_pos, owner) = (base.unwrap_or(src.position), src.player);
        // `ObjectCreationList.cpp:1313`: `RequiresLivePlayer` OCLs do not spawn for a dead player
        // (`!getControllingPlayer()->isPlayerActive()`), so OCLs never give useful objects to a
        // player after `killPlayer`.
        if t.requires_live_player && !self.is_player_active(owner) {
            return;
        }
        for _ in 0..t.count {
            // `pick = GameLogicRandomValue(0, m_names.size() - 1)`
            let pick = self.rng.value(0, t.names_len as i32 - 1) as usize;
            let tmpl = if t.is_object {
                t.objects.get(pick).and_then(|o| o.clone())
            } else {
                // CreateDebris always uses `GenericDebris`; `pick` was still drawn above.
                t.objects.first().and_then(|o| o.clone())
            };
            let Some(tmpl) = tmpl else { continue };

            let mut pos = Coord3D::new(src_pos.x + t.offset.0, src_pos.y + t.offset.1, src_pos.z + t.offset.2);
            // `doStuffToObj`'s `LIKE_EXISTING` (`ObjectCreationList.cpp:1038-1043`) orients the
            // object by the caller's angle (`INVALID_ANGLE` -> 0, `ObjectCreationList.cpp:799`). The
            // dispositions below overwrite it in retail's order, as they do here.
            let mut angle = if t.disposition & disposition::LIKE_EXISTING != 0 { build_angle.unwrap_or(0.0) } else { 0.0 };
            // The engine creates the object first (`TheThingFactory->newObject`, ObjectCreationList.cpp:1361
            // and :450/:466 for the other nuggets); `doStuffToObj` then draws for it. So the object's
            // construction draws (`create_object`) come before the draws below.
            let id = self.create_object(&tmpl, pos, angle, owner);
            // `doStuffToObj`'s `setInitialHealth`: `GameLogicRandomValueReal(m_minHealth, m_maxHealth)` (1,1 = no draw),
            // ObjectCreationList.cpp:993.
            let _health = self.rng.value_real(t.min_health, t.max_health);
            // `doStuffToObj`'s `sdu->onEnslave(sourceObj)` for a `SlavedUpdate` (ObjectCreationList.cpp:1000-1005), after
            // the health draw and before the dispositions. Lane 165: the same pinned-point draw as a spawn
            // (SlavedUpdate.cpp:712). The source object is the slave's master (`m_slaver`, `:706`).
            self.slaved_on_enslave(id, source);
            // `doStuffToObj`'s `LIKE_EXISTING` block (`ObjectCreationList.cpp:1040-1062`): a structure
            // stamps the terrain down underneath itself, re-reads the ground for its z, and only then
            // joins the pathfind map. (No draw, so its place in the order does not matter.)
            if t.disposition & disposition::LIKE_EXISTING != 0
                && self.objects.get(&id).is_some_and(|o| o.is_kind_of(crate::template::kind_of::STRUCTURE))
            {
                if let Some((p, a, template)) =
                    self.objects.get(&id).map(|o| (o.position, o.angle, o.template.clone()))
                {
                    self.terrain.flatten_terrain(&p, a, &template.geometry);
                    // `adjustedPos.z = getGroundHeight(pos->x, pos->y)` reads the base position, not the
                    // object's offset position (ObjectCreationList.cpp:1061).
                    let z = self.terrain.ground_height(src_pos.x, src_pos.y);
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.set_position(Coord3D::new(p.x, p.y, z));
                    }
                    self.pathfinder.classify_object_footprint(&self.terrain, id, &template, &p, a, true);
                }
            }
            // `doStuffToObj` effects applied after creation (`ObjectCreationList.cpp:1089-1225`).
            let mut force: Option<Coord3D> = None;
            let mut rates: Option<(f32, f32, f32)> = None;
            let mut allow_bouncing = false;

            // `doStuffToObj` dispositions, in the original's order.
            if t.disposition & disposition::ON_GROUND_ALIGNED != 0 {
                angle = self.rng.value_real(0.0, TWO_PI);
                pos.z = self.terrain.ground_height(pos.x, pos.y);
            }
            if t.disposition & disposition::SEND_IT_OUT != 0 {
                angle = self.rng.value_real(0.0, TWO_PI);
                pos.z = self.terrain.ground_height(pos.x, pos.y);
                let horiz = 4.0 * t.disposition_intensity;
                let fx = self.rng.value_real(-horiz, horiz);
                let fy = self.rng.value_real(-horiz, horiz);
                force = Some(Coord3D::new(fx, fy, 0.0));
                if t.orient_in_force_direction {
                    angle = atan2(fy, fx);
                }
            }
            if t.disposition & (disposition::SEND_IT_FLYING | disposition::SEND_IT_UP | disposition::RANDOM_FORCE) != 0 {
                let spin = if t.spin_rate >= 0.0 { t.spin_rate } else { (PI / 32.0) * t.disposition_intensity };
                let yaw_rate = if t.yaw_rate >= 0.0 { t.yaw_rate } else { spin };
                let roll_rate = if t.roll_rate >= 0.0 { t.roll_rate } else { spin };
                let pitch_rate = if t.pitch_rate >= 0.0 { t.pitch_rate } else { spin };
                let yaw = self.rng.value_real(-yaw_rate, yaw_rate);
                let roll = self.rng.value_real(-roll_rate, roll_rate);
                let pitch = self.rng.value_real(-pitch_rate, pitch_rate);
                rates = Some((yaw, roll, pitch));
                allow_bouncing = true;
                if t.disposition & disposition::SEND_IT_FLYING != 0 {
                    let horiz = 4.0 * t.disposition_intensity;
                    let vert = 3.0 * t.disposition_intensity;
                    let fx = self.rng.value_real(-horiz, horiz);
                    let fy = self.rng.value_real(-horiz, horiz);
                    let fz = self.rng.value_real(vert * 0.33, vert);
                    force = Some(Coord3D::new(fx, fy, fz));
                    if t.orient_in_force_direction {
                        angle = atan2(fy, fx);
                    }
                } else if t.disposition & disposition::SEND_IT_UP != 0 {
                    let horiz = 2.0 * t.disposition_intensity;
                    let vert = 4.0 * t.disposition_intensity;
                    let fx = self.rng.value_real(-horiz, horiz);
                    let fy = self.rng.value_real(-horiz, horiz);
                    let fz = self.rng.value_real(vert * 0.75, vert);
                    force = Some(Coord3D::new(fx, fy, fz));
                    if t.orient_in_force_direction {
                        angle = atan2(fy, fx);
                    }
                } else {
                    // `calcRandomForce`: angle, pitch, magnitude.
                    let force_angle = self.rng.value_real(-PI, PI);
                    let force_pitch = self.rng.value_real(t.min_force_pitch, t.max_force_pitch);
                    let force_mag = self.rng.value_real(t.min_force_magnitude, t.max_force_magnitude);
                    let (fx, fy, fz) = calc_random_force(force_angle, force_pitch, force_mag);
                    force = Some(Coord3D::new(fx, fy, fz));
                    if t.orient_in_force_direction {
                        angle = force_angle;
                    }
                }
            }
            if t.disposition & disposition::WHIRLING != 0 {
                let yaw = self.rng.value_real(-t.disposition_intensity, t.disposition_intensity);
                let roll = self.rng.value_real(-t.disposition_intensity, t.disposition_intensity);
                let pitch = self.rng.value_real(-t.disposition_intensity, t.disposition_intensity);
                rates = Some((yaw, roll, pitch));
            }
            if t.spread_formation {
                // `findPositionAround`: the first draw is the radius; the ring search itself is not
                // ported, so the object stays at the offset.
                let _radius = self.rng.value_real(t.min_distance_a_formation, t.min_distance_b_formation);
            }
            let _ = t.max_distance_formation;

            // `setOrientation` / `setPosition` of the dispositions (ground height for the aligned and sent-out kinds).
            if t.disposition & (disposition::ON_GROUND_ALIGNED | disposition::SEND_IT_OUT) != 0 {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.set_position(crate::math::Coord3D::new(o.position.x, o.position.y, pos.z));
                }
            }
            if let Some(o) = self.objects.get_mut(&id) {
                o.set_orientation(angle);
            }
            if let Some(f) = force {
                self.apply_force(id, f);
            }
            if let Some((yaw, roll, pitch)) = rates {
                self.set_angular_rates(id, yaw, roll, pitch);
            }
            if allow_bouncing {
                self.set_allow_bouncing(id, true);
            }
            self.set_ocl_producer(id, source);
        }
    }

    /// `ObjectCreationNugget`'s `sin`/`cos` helpers used by [`calc_random_force`] callers.
    #[allow(dead_code)]
    fn force_direction(force: (f32, f32)) -> f32 {
        atan2(force.1, force.0)
    }

    /// `GenericObjectCreationNugget`: a created object's producer is the OCL's source, and — when
    /// the object carries a `SpecialPowerCompletionDie` — that die's creator is the source too
    /// (`ObjectCreationList.cpp`).
    pub(crate) fn set_ocl_producer(&mut self, id: ObjectId, source: ObjectId) {
        let has_die = self.objects.get(&id).is_some_and(|o| o.template.has_special_power_completion);
        if let Some(o) = self.objects.get_mut(&id) {
            o.producer_id = source;
            if has_die && o.sp_creator == ObjectId::INVALID {
                o.sp_creator = source;
            }
        }
    }
}

/// The X vector of `Scale(mag) * Rotate_Z(angle) * Rotate_Y(-pitch)`, as `calcRandomForce` builds
/// it. Kept for reference/tests even though the sim does not apply the force.
pub fn calc_random_force(angle: f32, pitch: f32, mag: f32) -> (f32, f32, f32) {
    (mag * cos(angle) * cos(pitch), mag * sin(angle) * cos(pitch), mag * sin(pitch))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::{kind_of, Geometry, GeometryShape};
    use crate::weapon::{CombatTemplate, WeaponSetTemplate};
    use crate::{GameLogic, ObjectTemplate, TerrainLogic};

    fn armed_tank(name: &str, weapon: WeaponTemplate) -> ObjectTemplate {
        let mut t = ObjectTemplate::new(name);
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
        t.geometry.major_radius = 8.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.vision_range = 200.0;
        let mut w = WeaponSetTemplate::default();
        w.weapons[0] = Some(Arc::new(weapon));
        t.combat = CombatTemplate { weapon_set: Some(w), ..CombatTemplate::default() };
        t
    }

    #[test]
    fn attack_nugget_fires_the_requested_shots_and_stops() {
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        let mut w = WeaponTemplate::new("Cannon");
        w.primary_damage = 40.0;
        w.primary_damage_radius = 20.0;
        w.damage_type = crate::damage::DamageType::ArmorPiercing;
        w.attack_range = 200.0;
        w.min_delay_between_shots = 5;
        w.max_delay_between_shots = 5;
        let a = g.create_object(&Arc::new(armed_tank("A", w)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
        let mut b_t = ObjectTemplate::new("B");
        b_t.kind_of = kind_of::VEHICLE;
        b_t.geometry = Geometry { shape: GeometryShape::Cylinder, major_radius: 4.0, minor_radius: 4.0, height: 4.0, is_small: false };
        b_t.max_health = 100.0;
        b_t.initial_health = 100.0;
        let b = g.create_object(&Arc::new(b_t), Coord3D::new(250.0, 200.0, 0.0), 0.0, 1);
        let ocl = Arc::new(OclTemplate {
            name: "TestAttack".into(),
            nuggets: vec![OclNugget::Attack(AttackTemplate { number_of_shots: 1, weapon_slot: 0 })],
        });
        g.run_ocl_at(&ocl, a, Some(Coord3D::new(250.0, 200.0, 0.0)), false);
        // `run_ocl_at` sets the order; the shot needs a few frames (aim -> fire).
        g.run_frames(5);
        let hp = g.find_object(b).map_or(0.0, |o| o.health());
        assert_eq!(hp, 60.0, "one shot of 40 damage landed");
        // The one-shot limit ends the attack.
        g.run_frames(20);
        assert!(g.find_object(a).unwrap().combat.attack.is_none(), "attack ended after NumberOfShots");
        assert_eq!(g.find_object(b).unwrap().health(), 60.0, "no further shots");
    }

    #[test]
    fn apply_random_force_sets_velocity_and_rates() {
        let run = |seed: u32| {
            let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), seed);
            let mut t = ObjectTemplate::new("Chunk");
            t.kind_of = kind_of::VEHICLE;
            t.geometry.major_radius = 3.0;
            t.max_health = 50.0;
            t.initial_health = 50.0;
            t.physics = Some(crate::physics::PhysicsTemplate { mass: 1.0, ..Default::default() });
            let id = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 20.0), 0.0, 0);
            let ocl = Arc::new(OclTemplate {
                name: "TestForce".into(),
                nuggets: vec![OclNugget::ApplyRandomForce(ApplyRandomForceTemplate {
                    spin_rate: 0.1,
                    min_force_magnitude: 5.0,
                    max_force_magnitude: 5.0,
                    min_force_pitch: 0.0,
                    max_force_pitch: 0.0,
                })],
            });
            g.run_ocl_at(&ocl, id, None, false);
            let p = g.find_object(id).unwrap().physics.as_ref().unwrap();
            assert!(!p.allow_to_fall || p.accel != Coord3D::ZERO, "force became acceleration");
            assert!(p.yaw_rate != 0.0 || p.roll_rate != 0.0 || p.pitch_rate != 0.0, "spin rates set");
            let crc = g.crc();
            // The force integrates into velocity on the next physics update. A `PhysicsBehavior`'s
            // first update is one frame after creation (`PhysicsUpdate.cpp:231-233`), so that is the
            // frame after this one.
            g.update();
            g.update();
            let v = g.find_object(id).unwrap().physics.as_ref().unwrap().velocity;
            assert!(v.x != 0.0 || v.y != 0.0 || v.z != 0.0, "velocity integrated");
            crc
        };
        // Deterministic: the same seed draws the same force and rates.
        assert_eq!(run(7), run(7));
    }
}
