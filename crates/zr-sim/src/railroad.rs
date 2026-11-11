//! `RailroadBehavior` (`GameLogic/Object/Update/AIUpdate/RailroadGuideAIUpdate.cpp`): a locomotive
//! follows the map's waypoint track, pulls its carriages behind it and the carriages ride the same
//! track.
//!
//! The module lives on every object with a `RailroadBehavior` (the locomotive and each carriage).
//! Its first update on a locomotive builds its [`TrainTrack`] from the waypoints (`loadTrackData`,
//! :498-634) and creates/hitches its carriages (`createCarriages`, :940-1031); every update after
//! that moves the whole train (`update`, :670-851) by pulling its trailer recursively
//! (`getPulled`, :1170-1218), each car placed on the track by `FindPosByPathDistance`
//! (:1377-1510) and aligned by `updatePositionTrackDistance` (:1255-1341).
//!
//! `RailroadBehavior` derives from `PhysicsBehavior`, so it is also the train's `CollideModule`:
//! [`GameLogic::railroad_on_collide`] ports `onCollide` (:210-420) — the trample/crush/shove of
//! whatever the locomotive runs into and the rear-end crash of two trains — and `isRailroad`
//! (:188-205) decides which train cars are "ends" for that crash. Its `PhysicsBehaviorModuleData`
//! half (the block's `Mass`, `AllowBouncing`, ...) is parsed and carried in
//! [`crate::template::Railroad::physics`], but `Object::getPhysics()` is null for a train
//! (`Object.cpp:445-449` only fills `m_physics` for the module named `PhysicsBehavior`), and
//! `RailroadBehavior::update` replaces `PhysicsBehavior::update`, so the train has no physics module
//! from anyone else's point of view and its own physics state is never integrated.
//!
//! At a station the stopped train is a temporary pathfinding obstacle for every car
//! (`makeAWallOutOfThisTrain`, :640-663, called on `APPLY_BRAKES` -> `WAIT_AT_STATION` :713 and
//! again on the restart :733).
//!
//! Not ported: the passenger evacuation of `disembark` (:859-880; the flag and the
//! `APPLY_BRAKES` state are ported), the `m_held` script flag and the save/load (`xfer`) path. The
//! audio events (impact/whistle/running sounds), the `MODELCONDITION_OVER_WATER` drawable state and
//! the `Drawable::setDrawableHidden` call are client-only; only the hidden flag the viewer reads is
//! kept (`Object::drawable_hidden`).
//!
//! Ported from `GeneralsMD/Code/GameEngine/Source/GameLogic/Object/Update/AIUpdate/RailroadGuideAIUpdate.cpp`
//! (line numbers above are that file) and its header `.../Include/GameLogic/Module/RailroadGuideAIUpdate.h`.

use crate::damage::Relationship;
use crate::logic::GameLogic;
use crate::math::{angle_diff, atan2, cos, sin, sqrt, Coord3D};
use crate::module::{Request, SleepyUpdatePhase, UpdateContext, UpdateModule, UpdateSleepTime, UPDATE_SLEEP_NONE};
use crate::object::ObjectId;
use crate::partition::DistanceMode;
use crate::template::kind_of;
use std::sync::Arc;

/// `Coord3D::normalize` (`Core/Libraries/Include/Lib/Coord3D.h:38-48`): divide by the length unless
/// it is zero.
fn normalize(v: &mut Coord3D) {
    let len = sqrt(v.x * v.x + v.y * v.y + v.z * v.z);
    if len != 0.0 {
        v.x /= len;
        v.y /= len;
        v.z /= len;
    }
}

/// `Coord3D::scale` (`Coord3D.h:121-126`).
fn scale(v: &mut Coord3D, s: f32) {
    v.x *= s;
    v.y *= s;
    v.z *= s;
}

/// `0xfacade` (`RailroadGuideAIUpdate.cpp:110`, `:113`, `:118`, `:121`): the "no point yet" handle
/// `m_currentPointHandle` / `m_mostRecentSpecialPointHandle` start on.
const NO_POINT_HANDLE: i32 = 0xfacade;
/// `INVALID_WAYPOINT_ID` (`Core/GameEngine/Include/GameLogic/TerrainLogic.h:54`).
const INVALID_WAYPOINT_ID: i32 = 0x7fff_ffff;
/// `FRAMES_UNPULLED_LONG_ENOUGH_TO_UNHITCH` (`:65`).
const FRAMES_UNPULLED: i32 = 2;
/// `INVALID_ID`.
const INVALID_ID: ObjectId = ObjectId::INVALID;

/// `ConductorState` (`RailroadGuideAIUpdate.h:243-249`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConductorState {
    ApplyBrakes,
    WaitAtStation,
    Accelerate,
    Coast,
}

/// `RailroadBehavior::PullInfo` (`RailroadGuideAIUpdate.h:199-211`). A plain value: `getPulled`
/// copies its puller's info into its own `conductor`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PullInfo {
    /// `m_direction`: 1 = forward, -1 = backward (flipped at a `PingPong` point).
    pub direction: f32,
    pub speed: f32,
    pub track_distance: f32,
    pub tow_hitch_position: Coord3D,
    pub most_recent_special_point_handle: i32,
    pub previous_waypoint: u32,
    pub current_waypoint: u32,
}

impl Default for PullInfo {
    fn default() -> Self {
        // `RailroadBehavior::RailroadBehavior` (:108-122).
        PullInfo {
            direction: 1.0,
            speed: 0.0,
            track_distance: 0.0,
            tow_hitch_position: Coord3D::ZERO,
            most_recent_special_point_handle: NO_POINT_HANDLE,
            previous_waypoint: NO_POINT_HANDLE as u32,
            current_waypoint: NO_POINT_HANDLE as u32,
        }
    }
}

/// `TrackPoint` (`RailroadGuideAIUpdate.h:107-145`).
#[derive(Clone, Copy, Debug)]
pub struct TrackPoint {
    pub position: Coord3D,
    /// `m_distanceFromPrev`.
    pub distance_from_prev: f32,
    /// `m_distanceFromFirst`: the arc length from the anchor to this point.
    pub distance_from_first: f32,
    /// `m_handle`: the *previous* waypoint's id (see `loadTrackData`).
    pub handle: i32,
    pub is_first_point: bool,
    pub is_last_point: bool,
    /// `m_isTunnelOrBridge`: the waypoint's name ends with `Tunnel` (bridges are named so too).
    pub is_tunnel_or_bridge: bool,
    pub is_station: bool,
    pub is_disembark: bool,
    pub is_ping_pong: bool,
}

/// `TrainTrack` (`RailroadGuideAIUpdate.h:148-183`): the waypoint track the whole train shares.
#[derive(Clone, Debug, Default)]
pub struct TrainTrack {
    pub points: Vec<TrackPoint>,
    pub is_looping: bool,
    pub length: f32,
}

/// The per-object `RailroadBehavior` state. The C++ keeps this in the update module; the port keeps
/// it here so one object's update can reach another's (the pull chain), keyed by object id in
/// `GameLogic::railroad_states`.
#[derive(Clone, Debug)]
pub struct RailroadState {
    pub track: Option<Arc<TrainTrack>>,
    /// `m_trailerID`: the object I am directly pulling.
    pub trailer: ObjectId,
    /// `conductorPullInfo`: the info that pulls me (copied from my puller in `getPulled`).
    pub conductor: PullInfo,
    /// `m_pullInfo`: my own info, handed to my trailer.
    pub pull: PullInfo,
    /// `m_currentPointHandle`.
    pub current_point_handle: i32,
    /// `m_waitAtStationTimer`. `UnsignedInt` (`RailroadGuideAIUpdate.h:85`): `--t` on 0 wraps, and
    /// `t <= 0` is `t == 0`.
    pub wait_at_station_timer: u32,
    /// `m_carriagesCreated`.
    pub carriages_created: bool,
    /// `m_trackDataLoaded`: `loadTrackData` is tried once.
    pub track_data_loaded: bool,
    /// `m_waitingInWings`: I have not entered the real track yet (a non-looping track only).
    pub waiting_in_wings: bool,
    /// `m_endOfLine`: I reached the end of a non-looping track.
    pub end_of_line: bool,
    /// `m_isLocomotive`.
    pub is_locomotive: bool,
    /// `m_isLeadCarriage`.
    pub is_lead_carriage: bool,
    /// `m_wantsToBeLeadCarriage`: counts frames without a pull; above
    /// [`FRAMES_UNPULLED`] the car takes over as the lead.
    pub wants_to_be_lead_carriage: i32,
    /// `m_disembark`: stop at the next station and evacuate (never set on the shipped maps: no
    /// `Disembark` waypoint exists there, and `disembark` itself is not ported).
    pub disembark: bool,
    /// `m_inTunnel`: do not snap the position to the ground height until the next waypoint.
    pub in_tunnel: bool,
    /// `m_held`: a script keeps the locomotive at its station (`setHeld`; no script action in the
    /// port sets it).
    pub held: bool,
    /// `m_conductorState`.
    pub conductor_state: ConductorState,
    /// `m_anchorWaypointID`.
    pub anchor_waypoint_id: i32,
}

impl RailroadState {
    /// `RailroadBehavior::RailroadBehavior` (:100-165). The heading is the object's transform (`Thing::m_transform`),
    /// read and written by the turn, not kept here.
    pub fn new(is_locomotive: bool) -> Self {
        RailroadState {
            track: None,
            trailer: INVALID_ID,
            conductor: PullInfo::default(),
            pull: PullInfo::default(),
            current_point_handle: NO_POINT_HANDLE,
            wait_at_station_timer: 0,
            carriages_created: false,
            track_data_loaded: false,
            waiting_in_wings: true,
            end_of_line: false,
            is_locomotive,
            is_lead_carriage: is_locomotive,
            wants_to_be_lead_carriage: 0,
            disembark: false,
            in_tunnel: false,
            held: false,
            conductor_state: if is_locomotive { ConductorState::Accelerate } else { ConductorState::Coast },
            anchor_waypoint_id: INVALID_WAYPOINT_ID,
        }
    }

    /// `FindPosByPathDistance` (`:1377-1510`): the track position `dist` along the track, and the
    /// state a locomotive reads off the point it is passing (`setState`).
    fn find_pos_by_path_distance(&mut self, dist: f32, length: f32, set_state: bool) -> Coord3D {
        let Some(track) = self.track.clone() else { return Coord3D::ZERO };
        self.waiting_in_wings = false;

        let mut actual_distance = dist;
        if track.is_looping {
            while actual_distance < 0.0 {
                actual_distance += length;
            }
            while actual_distance > length {
                actual_distance -= length;
            }
        } else {
            if dist < 0.0 {
                self.waiting_in_wings = true;
            } else if dist >= length {
                self.end_of_line = true;
            }
            // `actualDistance = MAX(MIN(length,dist),0)`: the branch assignments above are
            // overwritten in C++ too; only the flags survive.
            actual_distance = length.min(dist).max(0.0);
        }

        let mut pos = Coord3D::ZERO;
        for i in 0..track.points.len() {
            let this_point = track.points[i];
            if this_point.distance_from_first < actual_distance {
                let this_pos = this_point.position;
                let next = track.points.get(i + 1).copied();
                if let Some(next_point) = next.filter(|n| n.distance_from_first > actual_distance) {
                    let handle = this_point.handle;
                    let edge = self.current_point_handle != handle;
                    if set_state {
                        self.in_tunnel = this_point.is_tunnel_or_bridge;
                        if self.is_locomotive {
                            // THE LOCOMOTIVE SNIFFS THE TRACK TO SMELL FOR THE NEXT STATION AND TUNNEL
                            if edge {
                                self.current_point_handle = handle;
                                if this_point.is_station {
                                    self.conductor_state = ConductorState::ApplyBrakes;
                                    self.disembark = false;
                                } else if this_point.is_disembark {
                                    self.conductor_state = ConductorState::ApplyBrakes;
                                    self.disembark = true;
                                } else if this_point.is_ping_pong
                                    && self.conductor.most_recent_special_point_handle != handle
                                {
                                    self.conductor.most_recent_special_point_handle = handle;
                                    self.conductor_state = ConductorState::ApplyBrakes;
                                    self.disembark = false;
                                    self.conductor.direction = -self.conductor.direction;
                                }
                            }
                        }
                        // `edge && !m_inTunnel`: the clickety-clack audio event (client-only).
                    }
                    let difference = actual_distance - this_point.distance_from_first;
                    let mut delta =
                        Coord3D::new(next_point.position.x - this_pos.x, next_point.position.y - this_pos.y, next_point.position.z - this_pos.z);
                    let len = sqrt(delta.x * delta.x + delta.y * delta.y + delta.z * delta.z);
                    if len != 0.0 {
                        delta.x /= len;
                        delta.y /= len;
                        delta.z /= len;
                    }
                    return Coord3D::new(
                        this_pos.x + delta.x * difference,
                        this_pos.y + delta.y * difference,
                        this_pos.z + delta.z * difference,
                    );
                }
                pos = this_pos;
            }
        }
        pos
    }

    /// `updatePositionTrackDistance` (`:1255-1341`): place this car `2 * majorRadius` behind the
    /// puller on the track, turn it to face its puller's tow point and snap its z to the ground
    /// (unless in a tunnel). `puller` is the puller's own pull info; the returned value is the new
    /// `m_pullInfo` this car hands to its trailer.
    fn update_position_track_distance(&mut self, logic: &mut GameLogic, id: ObjectId, puller: PullInfo, mut my: PullInfo) -> PullInfo {
        let Some(track) = self.track.clone() else { return my };
        let Some((obj_pos, obj_angle, hitch_radius, obj_transform)) = logic
            .objects
            .get(&id)
            .map(|o| (o.position, o.angle, o.template.geometry.major_radius, o.transform))
        else {
            return my;
        };

        let length = track.length;
        my.track_distance = puller.track_distance - (hitch_radius * 2.0);
        my.speed = puller.speed;
        my.direction = puller.direction;

        my.tow_hitch_position = self.find_pos_by_path_distance(my.track_distance, length, false);
        let car_position = self.find_pos_by_path_distance(my.track_distance, length, true);

        // The turn point: the car's rear hitch point, on the ground unless in a tunnel.
        let mut turn_pos = obj_pos;
        if !self.in_tunnel {
            turn_pos.z = logic.terrain.ground_height(turn_pos.x, turn_pos.y);
        }
        let dir = Coord3D::new(cos(obj_angle), sin(obj_angle), 0.0);
        turn_pos.x += dir.x * -hitch_radius;
        turn_pos.y += dir.y * -hitch_radius;

        let dx = puller.tow_hitch_position.x - turn_pos.x;
        let dy = puller.tow_hitch_position.y - turn_pos.y;
        let desired_angle = atan2(dy, dx);
        // `Real relAngle = stdAngleDiff(desiredAngle, obj->getTransformMatrix()->Get_Z_Rotation());` (RailroadGuideAIUpdate.cpp:1306).
        let rel_angle = angle_diff(desired_angle, obj_transform.z_rotation());

        // `tmp(1); tmp.Translate(turnPos.x, turnPos.y, 0); tmp.Translate(trackPosDelta.x, trackPosDelta.y, 0);
        //  tmp.In_Place_Pre_Rotate_Z(relAngle); tmp.Translate(-turnPos.x, -turnPos.y, 0); mtx.mul(tmp, *transform)`
        // (RailroadGuideAIUpdate.cpp:1309-1318), then `setTransformMatrix(&mtx)` (:1328).
        let mut tmp = crate::transform::Matrix3D::identity();
        tmp.translate(turn_pos.x, turn_pos.y, 0.0);
        tmp.translate(car_position.x - turn_pos.x, car_position.y - turn_pos.y, 0.0);
        tmp.in_place_pre_rotate_z(rel_angle);
        tmp.translate(-turn_pos.x, -turn_pos.y, 0.0);
        let mtx = crate::transform::Matrix3D::mul(&tmp, &obj_transform);

        // `enforceElevation = getGroundHeight(turnPos)`; `if (!m_inTunnel) obj->setPositionZ(enforceElevation)` (:1323-1331).
        let ground = logic.terrain.ground_height(turn_pos.x, turn_pos.y);
        let in_tunnel = self.in_tunnel;
        if let Some(o) = logic.objects.get_mut(&id) {
            o.set_transform_matrix(mtx);
            if !in_tunnel {
                o.set_position(Coord3D::new(o.position.x, o.position.y, ground));
            }
        }
        my
    }
}

/// The locomotive's and every carriage's `RailroadBehavior` update module. The state lives in
/// `GameLogic::railroad_states` (the pull chain crosses objects), so the module only asks
/// `GameLogic` to run one update (`Request::RailroadUpdate`), exactly where the C++ module would
/// have run in the sleepy-update heap.
#[derive(Default)]
pub struct RailroadUpdate;

impl UpdateModule for RailroadUpdate {
    fn update(&mut self, ctx: &mut UpdateContext) -> UpdateSleepTime {
        ctx.requests.push(Request::RailroadUpdate(ctx.object.id));
        UPDATE_SLEEP_NONE
    }

    /// `RailroadBehavior` derives from `PhysicsBehavior`, whose `getUpdatePhase` is `PHASE_PHYSICS`
    /// (PhysicsUpdate.h:220), so the train's update is in phase 1 of its frame (lane 99: the C++ heap key
    /// of a train at f1163 is 4653, `wake << 2 | 1`).
    fn phase(&self) -> SleepyUpdatePhase {
        SleepyUpdatePhase::Physics
    }

    fn name(&self) -> &'static str {
        "RailroadBehavior"
    }
}

impl GameLogic {
    /// `RailroadBehavior::update` (`RailroadGuideAIUpdate.cpp:670-851`) for one object.
    pub(crate) fn railroad_update(&mut self, id: ObjectId) {
        let Some(st) = self.railroad_states.remove(&id) else { return };
        let st = self.railroad_update_object(id, st);
        self.railroad_states.insert(id, st);
    }

    fn railroad_update_object(&mut self, id: ObjectId, mut st: RailroadState) -> RailroadState {
        // `if (m_trackDataLoaded == FALSE && m_isLocomotive)`: load the track and create the
        // carriages on the first update.
        if !st.track_data_loaded && st.is_locomotive {
            st = self.railroad_load_track_data(id, st);
            if st.track.is_some() {
                self.railroad_create_carriages(id, &mut st);
            }
            st.track_data_loaded = true;
        }
        if st.track.is_none() {
            return st;
        }

        let Some(rail) = self.objects.get(&id).and_then(|o| o.template.railroad.clone()) else { return st };
        let (speed_max, acceleration, braking, friction) = (rail.speed_max, rail.acceleration, rail.braking, rail.friction);
        let wait_at_station_time = rail.wait_at_station_time;

        if st.is_locomotive {
            match st.conductor_state {
                ConductorState::ApplyBrakes => {
                    st.conductor.speed *= braking;
                    if st.conductor.speed.abs() < 0.1 {
                        st.conductor.speed = 0.0;
                        st.wait_at_station_timer = wait_at_station_time;
                        st.conductor_state = ConductorState::WaitAtStation;
                        // `makeAWallOutOfThisTrain(TRUE)` (:713): the stopped train is an obstacle
                        // until it departs.
                        self.railroad_make_wall_out_of_this_train(id, st.trailer, true);
                        if st.disembark {
                            // `disembark()` (:859) is not ported; the flag is cleared as C++ does.
                            st.disembark = false;
                        }
                    }
                }
                ConductorState::WaitAtStation => {
                    // `--m_waitAtStationTimer; if (m_waitAtStationTimer <= 0 && !m_held)`: the timer
                    // is an `UnsignedInt` (`RailroadGuideAIUpdate.h:85`), so the test is `== 0` and a
                    // `WaitAtStationTime` of 0 wraps to `0xffffffff` (the locomotive then never
                    // departs).
                    st.wait_at_station_timer = st.wait_at_station_timer.wrapping_sub(1);
                    if st.wait_at_station_timer == 0 && !st.held {
                        st.conductor_state = ConductorState::Accelerate;
                        st.conductor.speed = 0.05 * st.conductor.direction;

                        // `makeAWallOutOfThisTrain(FALSE)` (:733): the train stops blocking the
                        // pathfinder before it moves.
                        self.railroad_make_wall_out_of_this_train(id, st.trailer, false);
                    }
                    // `else if (timer == waitAtStationTime/4)`: the whistle (client-only).
                }
                ConductorState::Accelerate => {
                    st.conductor.speed += 0.02 * st.conductor.direction; // push start multiplier
                    st.conductor.speed *= acceleration;
                    if st.conductor.speed > speed_max {
                        st.conductor.speed = speed_max;
                    } else if st.conductor.speed < -speed_max {
                        st.conductor.speed = -speed_max;
                    }
                }
                ConductorState::Coast => {}
            }
        }

        // `if (m_wantsToBeLeadCarriage > FRAMES_UNPULLED...)`: I have lost my puller.
        if st.wants_to_be_lead_carriage > FRAMES_UNPULLED {
            st.is_lead_carriage = true;
        }

        let length = st.track.as_ref().map_or(0.0, |t| t.length);
        let is_looping = st.track.as_ref().is_some_and(|t| t.is_looping);

        if st.is_lead_carriage {
            if st.conductor_state == ConductorState::Coast {
                st.conductor.speed *= friction;
            }
            st.conductor.track_distance += st.conductor.speed;
            // Only a looping track normalizes the track position; a straight one lets the train
            // exit by exceeding the track length.
            while st.conductor.track_distance > length && is_looping {
                st.conductor.track_distance -= length;
            }
            while st.conductor.track_distance < 0.0 && is_looping {
                st.conductor.track_distance += length;
            }

            st.conductor.tow_hitch_position = st.find_pos_by_path_distance(st.conductor.track_distance, length, false);
            let conductor = st.conductor;
            let pull = st.pull;
            st.pull = st.update_position_track_distance(self, id, conductor, pull);

            // Pull the trailer with my own info.
            let trailer = st.trailer;
            if self.objects.get(&trailer).is_some_and(|o| !o.destroyed) {
                self.railroad_get_pulled(trailer, &mut st.pull);
            } else {
                st.trailer = INVALID_ID;
                if st.end_of_line {
                    self.destroy_object(id);
                }
            }
        } else if st.wants_to_be_lead_carriage <= FRAMES_UNPULLED {
            // Like every young carriage, I aspire to be the lead carriage some day.
            st.wants_to_be_lead_carriage += 1;
        }

        // `draw->setDrawableHidden(m_waitingInWings || m_endOfLine)` (:831-837): only for a
        // non-looping track. The `OVER_WATER` model condition below it is client-only.
        if !is_looping {
            let hidden = st.waiting_in_wings || st.end_of_line;
            if let Some(o) = self.objects.get_mut(&id) {
                o.drawable_hidden = hidden;
            }
        }

        st
    }

    /// `RailroadBehavior::makeAWallOutOfThisTrain` (`RailroadGuideAIUpdate.cpp:640-663`): turn the
    /// footprint of this car (and, recursively, every car behind it) into a temporary pathfinding
    /// obstacle (`Pathfinder::createAWallFromMyFootprint` / `removeWallFromMyFootprint`,
    /// `AIPathfind.h:181-182`), so the stopped train blocks paths until it departs.
    ///
    /// The engine's warning "The object MUST NOT MOVE between the call to createAWall... and
    /// removeWall..." (`AIPathfind.h:178-180`) holds: the calls sit where the train stops and where it
    /// restarts, before either frame moves it.
    ///
    /// `trailer` is this object's `m_trailerID`: the caller's own state is detached from
    /// `railroad_states` while its update runs, so the first hop comes in as an argument.
    fn railroad_make_wall_out_of_this_train(&mut self, id: ObjectId, trailer: ObjectId, on: bool) {
        // The C++ recursion follows `m_trailerID` into the trailer's `RailroadBehavior` module; the
        // cap is a safety net for a malformed trailer cycle (the original would recurse forever).
        let mut next = Some((id, trailer));
        for _ in 0..=self.objects.len() {
            let Some((cur, trailer)) = next else { break };
            let Some((geometry, pos, angle)) = self.objects.get(&cur).map(|o| (o.template.geometry, o.position, o.angle)) else {
                break;
            };
            if on {
                self.pathfinder.create_wall_from_footprint(cur, &geometry, &pos, angle);
            } else {
                self.pathfinder.remove_wall_from_footprint(cur, &geometry, &pos, angle);
            }
            next = if trailer != INVALID_ID {
                self.railroad_states.get(&trailer).map(|s| (trailer, s.trailer))
            } else {
                None
            };
        }
    }

    /// `RailroadBehavior::getPulled` (`:1170-1218`): take the puller's info, place myself behind it
    /// and hand my own info down the chain.
    fn railroad_get_pulled(&mut self, id: ObjectId, info: &mut PullInfo) {
        let Some(mut st) = self.railroad_states.remove(&id) else { return };
        // ENFORCE MY STATUS AS A PULLEE, NOT A PULLER.
        st.wants_to_be_lead_carriage = 0;
        if st.track.is_none() {
            self.railroad_states.insert(id, st);
            return;
        }
        st.conductor = *info;
        info.previous_waypoint = info.current_waypoint;

        let pull = st.pull;
        st.pull = st.update_position_track_distance(self, id, *info, pull);

        let trailer = st.trailer;
        if self.objects.get(&trailer).is_some_and(|o| !o.destroyed) {
            self.railroad_get_pulled(trailer, &mut st.pull);
        } else {
            st.trailer = INVALID_ID;
            if st.end_of_line {
                self.destroy_object(id);
            }
        }
        self.railroad_states.insert(id, st);
    }

    /// `RailroadBehavior::isRailroad` (`RailroadGuideAIUpdate.cpp:188-205`): a pair of trains only
    /// count as a rear-end crash at the ends of the train — a lead carriage (`m_isLeadCarriage`, the
    /// locomotive while it pulls) or the last car (`m_trailerID == INVALID_ID`). A car in the middle
    /// "refuses to cooperate" so that the locomotive does not kill its own train when the cars touch.
    pub(crate) fn railroad_is_railroad(&self, id: ObjectId) -> bool {
        let Some(st) = self.railroad_states.get(&id) else { return false };
        if st.track.is_none() {
            return false; // I haven't even started yet.
        }
        if st.waiting_in_wings || st.end_of_line {
            return false;
        }
        st.is_lead_carriage || st.trailer == INVALID_ID
    }

    /// `RailroadBehavior::onCollide` (`RailroadGuideAIUpdate.cpp:210-420`): the train's collide
    /// module. A locomotive at or above `KillSpeedMin` tramples what it hits (two
    /// `GameLogicRandomValueReal(-0.03, 0.03)` draws for the victim's pitch and roll). Below it, it
    /// crushes with `DAMAGE_CRUSH` for `speed * 10`. Either way the victim is shoved out of the
    /// train's way and thrown, and a lead carriage (or the locomotive) that runs into another train's
    /// end kills it.
    ///
    /// `theirPhys = other->getPhysics()` is the victim's physics. It is null for another train car:
    /// `Object::m_physics` is only filled for the module named `PhysicsBehavior`
    /// (`Object.cpp:445-449`), and a `RailroadBehavior` (`RailroadGuideAIUpdate.h:189-190`) is not
    /// it — so a train is insubstantial to `PhysicsBehavior::onCollide` and `SquishCollide::onCollide`
    /// too, and a locomotive's own cars never trigger the kill branch (the AN3 locomotive collides
    /// with its first car every frame at speed 5 and the C++ run has no Railroad RNG draw).
    pub(crate) fn railroad_on_collide(&mut self, id: ObjectId, other: ObjectId) {
        let Some(st) = self.railroad_states.get(&id).cloned() else { return };
        if !self.objects.contains_key(&other) {
            return; // sanity (the engine's `other` is never null here)
        }
        if st.waiting_in_wings || st.end_of_line {
            return;
        }
        // `for (BehaviorModule** m = other->getBehaviorModules(); *m; ++m) if (collide->isRailroad())`
        if self.railroad_is_railroad(other) {
            if st.is_locomotive {
                // yikes! I just rear ended a stranded train! Oh well...
                self.kill_object(other);
            } else if st.is_lead_carriage {
                // yikes! I just coasted into some other carriage
                self.kill_object(other);
                self.kill_object(id); // and I am dead, too
            }
            return; // its a train, folks
        }

        let Some((is_structure, faction_building, has_demo_trap, under_construction)) = self.objects.get(&other).map(|v| {
            (
                v.is_kind_of(kind_of::STRUCTURE),
                v.is_kind_of(kind_of::FS_POWER)
                    || v.is_kind_of(kind_of::FS_FACTORY)
                    || v.is_kind_of(kind_of::FS_BASE_DEFENSE)
                    || v.is_kind_of(kind_of::FS_TECHNOLOGY)
                    || v.is_kind_of(kind_of::REBUILD_HOLE),
                v.template.demo_trap.is_some(),
                v.under_construction,
            )
        }) else {
            return;
        };
        if is_structure {
            // Now be careful here: we ignore buildings, except for hostile, nasty ones that can blow us up.
            if faction_building {
                // `playImpactSound(other, other->getPosition())` is audio only.
                self.kill_object(other);
                return;
            }
            // A DemoTrap can detonate on the train, but only once it is built.
            if has_demo_trap {
                if !under_construction {
                    self.kill_object(id); // it can only detonate on me if it is ready
                }
                self.kill_object(other);
                return;
            }
        }

        // `PhysicsBehavior* theirPhys = other->getPhysics(); if (!theirPhys) return;`
        if self.objects.get(&other).and_then(|o| o.template.physics).is_none() {
            return;
        }
        let Some((running_garrison_speed_max, kill_speed_min)) =
            self.objects.get(&id).and_then(|o| o.template.railroad.as_ref()).map(|r| (r.running_garrison_speed_max, r.kill_speed_min))
        else {
            return;
        };

        // Maybe we don't want to trample this unit? They can grab on safely at a station.
        if st.conductor_state == ConductorState::WaitAtStation && st.pull.speed < running_garrison_speed_max {
            let intends_to_board = self.objects.get(&other).is_some_and(|o| o.enter_target == Some(id));
            if intends_to_board {
                return;
            }
        }
        // I don't mash my own sweet passengers.
        if self.objects.get(&other).is_some_and(|o| o.contained_by == Some(id)) {
            return;
        }

        let Some((victim_is_infantry, their_radius, their_pos)) = self
            .objects
            .get(&other)
            .map(|v| (v.is_kind_of(kind_of::INFANTRY), v.template.geometry.major_radius, v.position))
        else {
            return;
        };
        let Some((my_radius, my_loc, my_angle)) = self.objects.get(&id).map(|o| (o.template.geometry.major_radius, o.position, o.angle)) else {
            return;
        };
        let mut their_loc = their_pos;

        // If we made it this far, it is something we don't want to share space with.
        let mut dlt = Coord3D::new(their_loc.x - my_loc.x, their_loc.y - my_loc.y, their_loc.z - my_loc.z);
        let dist = sqrt(dlt.x * dlt.x + dlt.y * dlt.y + dlt.z * dlt.z);
        let mut overlap = ((my_radius + their_radius) - dist) + 1.0; // the plus 1 makes them go just outside of me.
        normalize(&mut dlt);
        if !victim_is_infantry {
            overlap /= 4.0;
            scale(&mut dlt, overlap);
            let new_pos = Coord3D::new(their_loc.x + dlt.x, their_loc.y + dlt.y, their_loc.z + dlt.z);
            if let Some(o) = self.objects.get_mut(&other) {
                o.set_position(new_pos);
            }
            // `theirLoc` is a pointer in C++: the position reads below see the new spot.
            their_loc = new_pos;
        }

        if st.conductor_state == ConductorState::WaitAtStation
            || (st.conductor_state == ConductorState::Coast && st.pull.speed < running_garrison_speed_max)
            || !st.is_locomotive
        {
            return; // let those trying to board pass through unhindered
        }

        // Figure out the relative slope between them and me.
        let mut delta = Coord3D::new(their_loc.x - my_loc.x, their_loc.y - my_loc.y, their_loc.z - my_loc.z);
        normalize(&mut delta);
        let (dx, dy) = (cos(my_angle), sin(my_angle));
        let dot = delta.x * dx + delta.y * dy; // `myDir->z` is 0

        // `other->isEffectivelyDead()` (RailroadGuideAIUpdate.cpp:359): the port's analogue is `is_dying()`
        // (a 0-health `ActiveBody` is effectively dead too, ActiveBody.cpp:1255).
        if self.objects.get(&other).is_some_and(|o| o.is_dying()) {
            // We just run over debris, instead of shoving it around.
            scale(&mut delta, (st.pull.speed * 0.66).min(0.3));
        } else {
            scale(&mut delta, (st.pull.speed * 0.66).min(1.4)); // the faster I go, the harder I slam!
            if st.pull.speed >= kill_speed_min {
                // Absolute death to be hit by a train, no survival.
                self.kill_object(other);
                // Draw order: pitch then roll, each `GameLogicRandomValueReal(-0.03, 0.03)`.
                let pitch = self.rng.value_real(-0.03, 0.03);
                let roll = self.rng.value_real(-0.03, 0.03);
                if let Some(p) = self.objects.get_mut(&other).and_then(|o| o.physics.as_mut()) {
                    p.pitch_rate = pitch;
                    p.roll_rate = roll;
                }
            } else {
                // `playImpactSound(other, loc)` (a bounce sound) is audio only.
                let amount = st.pull.speed * 10.0; // hurt!
                self.damage_object_with_death(
                    other,
                    crate::body::DamageInfo { source: id, amount, kind: crate::damage::DamageType::Crush },
                    crate::damage::DeathType::Crushed,
                );
            }
        }

        // Lift them off the ground and shove them (`theirLoc` is a live pointer in C++).
        if let Some(p) = self.objects.get(&other).map(|o| o.position) {
            their_loc = p;
        }
        let mut heft = their_loc;
        heft.z = heft.z.max(self.terrain.ground_height(heft.x, heft.y) + 2.0);
        if let Some(o) = self.objects.get_mut(&other) {
            o.set_position(heft);
        }

        // `for some fake heft`: the z of the thrown velocity is random, then the whole vector takes
        // the relative-slope factor.
        delta.z = self.rng.value_real(0.05, st.pull.speed / 10.0);
        scale(&mut delta, dot);

        // This is a special check so that it won't hurl infantry clear across the map!
        let victim_speed = self.objects.get(&other).and_then(|o| o.physics.as_ref()).map_or(0.0, |p| p.velocity_magnitude());
        if !(victim_is_infantry && victim_speed > 5.0) {
            self.add_velocity_to(other, delta);
        }

        self.set_allow_to_fall(other, true);
        self.set_allow_bouncing(other, true);
        self.set_allow_airborne_friction(other, true);

        // `myDir->crossProduct(*myDir, up, cross)` with `up = (0,0,1)`.
        let (cross_x, cross_y) = (dy, -dx);
        normalize(&mut delta);
        let deviation = cross_x * delta.x + cross_y * delta.y;
        if dot > 0.0 {
            if let Some(p) = self.objects.get_mut(&other).and_then(|o| o.physics.as_mut()) {
                p.yaw_rate = deviation * -0.06 * st.pull.speed;
            }
        }
    }

    /// `RailroadBehavior::loadTrackData` (`:498-634`): the anchor is the map's nearest waypoint,
    /// and the track follows each waypoint's first link.
    fn railroad_load_track_data(&mut self, id: ObjectId, mut st: RailroadState) -> RailroadState {
        if st.track.is_some() {
            return st;
        }
        let Some(my_pos) = self.objects.get(&id).map(|o| o.position) else { return st };
        // `TheTerrainLogic->getFirstWaypoint()` null: no waypoint, no anchor, no track.
        if !self.scripts.has_waypoints() {
            return st;
        }

        // The nearest waypoint by 3D distance (`delta.length()`, strict `<` keeps the first on a tie).
        let mut anchor: Option<(f32, i32)> = None;
        for (&wid, w) in &self.scripts.waypoint_by_id {
            let d = sqrt(
                (my_pos.x - w.pos.x) * (my_pos.x - w.pos.x)
                    + (my_pos.y - w.pos.y) * (my_pos.y - w.pos.y)
                    + (my_pos.z - w.pos.z) * (my_pos.z - w.pos.z),
            );
            if anchor.is_none_or(|(best, _)| best > d) {
                anchor = Some((d, wid));
            }
        }
        let Some((_, anchor_id)) = anchor else { return st };
        st.anchor_waypoint_id = anchor_id;

        let ends_with = |s: &str, suffix: &str| s.len() >= suffix.len() && s.ends_with(suffix);
        let first = &self.scripts.waypoint_by_id[&anchor_id];
        let mut points = Vec::new();
        points.push(TrackPoint {
            position: first.pos,
            distance_from_prev: 0.0,
            distance_from_first: 0.0,
            handle: anchor_id,
            is_first_point: true,
            is_last_point: false,
            is_tunnel_or_bridge: ends_with(&first.name, "Tunnel"),
            is_station: ends_with(&first.name, "Station"),
            is_disembark: ends_with(&first.name, "Disembark"),
            is_ping_pong: false,
        });

        let mut length = 0.0f32;
        let mut is_looping = false;
        let mut scanner = anchor_id;
        // The C++ `while (scanner)` walk; the cap is a safety net for a malformed link cycle
        // (the original would spin forever), it cannot trigger on a well-formed track.
        let cap = self.scripts.waypoint_by_id.len() + 1;
        while let Some(target_id) = self.scripts.waypoint_links.get(&scanner).and_then(|l| l.first()).copied() {
            let Some(from) = self.scripts.waypoint_by_id.get(&scanner) else { break };
            let Some(target) = self.scripts.waypoint_by_id.get(&target_id) else { break };
            let (from_pos, from_name) = (from.pos, from.name.clone());
            let (to_pos, to_name) = (target.pos, target.name.clone());
            let dist_from_to = sqrt(
                (from_pos.x - to_pos.x) * (from_pos.x - to_pos.x)
                    + (from_pos.y - to_pos.y) * (from_pos.y - to_pos.y)
                    + (from_pos.z - to_pos.z) * (from_pos.z - to_pos.z),
            );
            length += dist_from_to;
            points.push(TrackPoint {
                position: to_pos,
                distance_from_prev: dist_from_to,
                distance_from_first: length,
                handle: scanner,
                is_first_point: false,
                is_last_point: !self.scripts.waypoint_links.get(&target_id).is_some_and(|l| !l.is_empty()),
                is_tunnel_or_bridge: ends_with(&to_name, "Tunnel"),
                is_station: ends_with(&to_name, "Station"),
                is_ping_pong: ends_with(&from_name, "PingPong"),
                is_disembark: ends_with(&to_name, "Disembark"),
            });
            scanner = target_id;
            if scanner == anchor_id {
                is_looping = true;
                break;
            }
            if points.len() > cap {
                break;
            }
        }

        st.track = Some(Arc::new(TrainTrack { points, is_looping, length }));
        st
    }

    /// `RailroadBehavior::createCarriages` (`:940-1031`).
    fn railroad_create_carriages(&mut self, id: ObjectId, st: &mut RailroadState) {
        if !st.is_locomotive {
            return;
        }
        let Some(loco) = self.objects.get(&id) else { return };
        let player = loco.player;
        let Some(rail) = loco.template.railroad.clone() else { return };
        let max_radius = loco.template.geometry.major_radius * 2.0;
        let dir = Coord3D::new(cos(loco.angle), sin(loco.angle), 0.0);
        let hitch = Coord3D::new(loco.position.x - dir.x * max_radius, loco.position.y - dir.y * max_radius, loco.position.z);
        let Some(track) = st.track.clone() else { return };

        // `xferCarriage` (my recorded trailer) or the closest free carriage behind me.
        let mut close = if st.trailer != INVALID_ID { self.objects.get(&st.trailer).filter(|o| !o.destroyed).map(|o| o.id) } else { None };
        if close.is_none() {
            close = self.railroad_close_carriage_at(&hitch, max_radius, id);
        }

        // `if (iter != list.end())` and `if (temp)`: with an empty list or an unresolved first
        // template nothing is hitched or created.
        let Some(first_name) = rail.carriage_templates.first() else {
            st.carriages_created = true;
            return;
        };
        let Some(tmpl) = self.find_template(first_name) else {
            st.carriages_created = true;
            return;
        };

        let first_carriage = match close {
            Some(c) => Some(c),
            None => Some(self.create_object(&tmpl, Coord3D::ZERO, 0.0, player)),
        };
        if let Some(first) = first_carriage {
            self.railroad_set_producer(first, id);
            st.trailer = first;
            if self.objects.get(&first).is_some_and(|o| o.template.railroad.is_some()) {
                if close.is_some() {
                    self.railroad_hitch_by_proximity(first, &track);
                } else {
                    // `hitchNewCarriagebyTemplate(..., ++iter, ...)`: the list from index 1.
                    self.railroad_hitch_by_template(id, &rail.carriage_templates, 1, first, &track);
                }
            }
        }
        st.carriages_created = true;
    }

    /// `RailroadBehavior::hitchNewCarriagebyTemplate` (`:1035-1090`): mark carriage `id` as hitched
    /// and on the track, then create and hitch the next name in the list behind it.
    fn railroad_hitch_by_template(&mut self, loco: ObjectId, list: &[String], index: usize, id: ObjectId, track: &Arc<TrainTrack>) {
        if !self.objects.contains_key(&loco) {
            return; // sanity: the locomotive is gone
        }
        self.railroad_inherit_track(id, track);
        if index >= list.len() {
            return;
        }
        let Some(tmpl) = self.find_template(&list[index]) else { return };
        let Some(player) = self.objects.get(&loco).map(|o| o.player) else { return };
        let car = self.create_object(&tmpl, Coord3D::ZERO, 0.0, player);
        self.railroad_set_producer(car, loco);
        if let Some(st) = self.railroad_states.get_mut(&id) {
            st.trailer = car;
        }
        if self.objects.get(&car).is_some_and(|o| o.template.railroad.is_some()) {
            self.railroad_hitch_by_template(loco, list, index + 1, car, track);
        }
    }

    /// `RailroadBehavior::hitchNewCarriagebyProximity` (`:1094-1164`): mark carriage `id` hitched
    /// and on the track, then hitch the next free allied carriage in reach behind it.
    fn railroad_hitch_by_proximity(&mut self, id: ObjectId, track: &Arc<TrainTrack>) {
        self.railroad_inherit_track(id, track);
        let Some(searcher) = self.objects.get(&id) else { return };
        let max_radius = searcher.template.geometry.major_radius * 2.0;
        let dir = Coord3D::new(cos(searcher.angle), sin(searcher.angle), 0.0);
        let hitch = Coord3D::new(searcher.position.x - dir.x * max_radius, searcher.position.y - dir.y * max_radius, searcher.position.z);

        let recorded = self.railroad_states.get(&id).map(|s| s.trailer).unwrap_or(INVALID_ID);
        let mut close = if recorded != INVALID_ID { self.objects.get(&recorded).filter(|o| !o.destroyed).map(|o| o.id) } else { None };
        if close.is_none() {
            close = self.railroad_close_carriage_at(&hitch, max_radius, id);
        }
        if let Some(close) = close {
            self.railroad_set_producer(close, id);
            if let Some(st) = self.railroad_states.get_mut(&id) {
                st.trailer = close;
            }
            if self.objects.get(&close).is_some_and(|o| o.template.railroad.is_some()) {
                self.railroad_hitch_by_proximity(close, track);
            }
        }
    }

    /// `m_track = track; m_track->incReference(); m_hasEverBeenHitched = TRUE;`
    /// (`hitchNewCarriageby*`, :1050-1053 / :1108-1111). `m_hasEverBeenHitched` is the object's
    /// `railroad_hitched` flag (hashed), the track is inherited from the locomotive.
    fn railroad_inherit_track(&mut self, car: ObjectId, track: &Arc<TrainTrack>) {
        if let Some(c) = self.objects.get_mut(&car) {
            c.railroad_hitched = true;
        }
        if let Some(st) = self.railroad_states.get_mut(&car) {
            st.track = Some(track.clone());
        }
    }

    /// `PartitionFilterIsValidCarriage` + `PartitionManager::getClosestObject(hitch, maxRadius,
    /// FROM_CENTER_2D)` (`:897-937`): the closest object to `hitch` in `max_radius` that has a
    /// `RailroadBehavior`, has never been hitched and is an ally of `searcher`'s owner.
    fn railroad_close_carriage_at(&self, hitch: &Coord3D, max_radius: f32, searcher: ObjectId) -> Option<ObjectId> {
        let searcher_player = self.objects.get(&searcher)?.player;
        self.partition
            .objects_in_range(hitch, max_radius, DistanceMode::FromCenter2D)
            .into_iter()
            .find(|&id| {
                id != searcher
                    && self.objects.get(&id).is_some_and(|c| {
                        !c.destroyed
                            && !c.railroad_hitched
                            && c.template.railroad.is_some()
                            && self.relationship(searcher_player, c.player) == Relationship::Allies
                    })
            })
    }

    fn railroad_set_producer(&mut self, car: ObjectId, producer: ObjectId) {
        if let Some(c) = self.objects.get_mut(&car) {
            c.producer_id = producer;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logic::GameLogic;
    use crate::template::{kind_of, ObjectTemplate, Railroad, SimpleTemplateStore};
    use crate::terrain::TerrainLogic;
    use std::sync::Arc;

    /// `RailroadBehavior` derives from `PhysicsBehavior`, whose update phase is `PHASE_PHYSICS` (PhysicsUpdate.h:220).
    /// Revert check: the default `PHASE_NORMAL` fails this.
    /// R18-9 (`RailroadGuideAIUpdate.cpp:1306-1331`): a carriage's turn reads and writes the object's transform
    /// (`getTransformMatrix`, `setTransformMatrix`), so after a turn its heading is the matrix readback. Before the fix
    /// the heading was tracked beside the transform and written as an angle, and the transform kept the old heading.
    /// Revert check: write `o.angle = atan2(mtx.row[1][0], mtx.row[0][0])` alone, as before. The revert also stops the
    /// lead car from building its carriage (the stale heading misplaces the first turn), so that run fails at setup.
    #[test]
    fn a_turn_writes_the_carriages_heading_into_its_transform() {
        let mut g = square_loop(vec![car("Loco", rail_car(true, &["CarA"]), 5.0), car("CarA", rail_car(false, &[]), 5.0)]);
        let l = spawn(&mut g, "Loco", Coord3D::new(0.0, 0.0, 0.0), 0.0);
        g.run_frames(2);
        let car_id = others(&g, l).into_iter().find(|m| m.0 == "CarA").expect("the carriage is on the train").1;
        let start = g.find_object(car_id).unwrap().angle;
        let mut frames = 0;
        while (g.find_object(car_id).unwrap().angle - start).abs() < 0.2 && frames < 4000 {
            g.run_frames(1);
            frames += 1;
        }
        let o = g.find_object(car_id).unwrap();
        assert!((o.angle - start).abs() >= 0.2, "the carriage turned with the train");
        assert_eq!(o.angle.to_bits(), o.transform.z_rotation().to_bits(), "its heading is the matrix readback");
    }

    #[test]
    fn a_train_update_runs_in_the_physics_phase() {
        assert_eq!(RailroadUpdate.phase(), SleepyUpdatePhase::Physics);
    }

    /// `RailroadBehavior` module data with the C++ constructor's defaults (`:87-94`) and, for a
    /// locomotive, a fast acceleration so a test needs few frames.
    fn rail_car(locomotive: bool, carriages: &[&str]) -> Option<Railroad> {
        Some(Railroad {
            is_locomotive: locomotive,
            carriage_templates: carriages.iter().map(|s| s.to_string()).collect(),
            ..Railroad::default()
        })
    }

    fn car(name: &str, railroad: Option<Railroad>, radius: f32) -> ObjectTemplate {
        let mut t = ObjectTemplate::new(name);
        t.kind_of = kind_of::VEHICLE;
        t.geometry.major_radius = radius;
        t.railroad = railroad;
        t
    }

    /// A world with `templates` and the given waypoints (id, name, position) and links.
    fn world(
        templates: Vec<ObjectTemplate>,
        waypoints: Vec<(i32, &str, Coord3D)>,
        links: &[(i32, i32)],
    ) -> GameLogic {
        world_on(TerrainLogic::flat(400, 400, 0), templates, waypoints, links)
    }

    fn world_on(
        terrain: TerrainLogic,
        templates: Vec<ObjectTemplate>,
        waypoints: Vec<(i32, &str, Coord3D)>,
        links: &[(i32, i32)],
    ) -> GameLogic {
        let mut store = SimpleTemplateStore::default();
        for t in templates {
            store.add(t);
        }
        let mut g = GameLogic::new(terrain, 1);
        g.set_template_source(Arc::new(store));
        if !waypoints.is_empty() {
            g.scripts_mut().set_waypoint_paths(
                waypoints.into_iter().map(|(id, n, p)| (id, n.to_owned(), p, Vec::new())),
                links.iter().copied(),
            );
        }
        g
    }

    /// A straight 3-point track along +x, 100 units per leg. `looping` links the last back to the
    /// first.
    fn straight_world(templates: Vec<ObjectTemplate>, looping: bool) -> GameLogic {
        let wps = vec![
            (1, "Start", Coord3D::new(0.0, 0.0, 0.0)),
            (2, "Middle", Coord3D::new(100.0, 0.0, 0.0)),
            (3, "End", Coord3D::new(200.0, 0.0, 0.0)),
        ];
        let mut links = vec![(1, 2), (2, 3)];
        if looping {
            links.push((3, 1));
        }
        world(templates, wps, &links)
    }

    /// A square loop 1000 units a side. On the short `straight_world` loop the seam puts a car's
    /// wrapped track distance back within a car's length of the locomotive, so the locomotive and
    /// its own last car overlap; a real track has room for that (this only matters for tests that
    /// keep carriages alive).
    fn square_loop(templates: Vec<ObjectTemplate>) -> GameLogic {
        let wps = vec![
            (1, "Start", Coord3D::new(0.0, 0.0, 0.0)),
            (2, "SideA", Coord3D::new(1000.0, 0.0, 0.0)),
            (3, "SideB", Coord3D::new(1000.0, 1000.0, 0.0)),
            (4, "SideC", Coord3D::new(0.0, 1000.0, 0.0)),
        ];
        world_on(TerrainLogic::flat(2000, 2000, 0), templates, wps, &[(1, 2), (2, 3), (3, 4), (4, 1)])
    }

    fn spawn(g: &mut GameLogic, name: &str, pos: Coord3D, angle: f32) -> ObjectId {
        let t = g.find_template(name).expect("template");
        g.create_object(&t, pos, angle, 0)
    }

    fn state(g: &GameLogic, id: ObjectId) -> RailroadState {
        match g.railroad_states.get(&id) {
            Some(s) => s.clone(),
            None => panic!("no railroad state for {id:?}; objects {:?}", g.objects().map(|o| (o.id, o.template.name.clone())).collect::<Vec<_>>()),
        }
    }

    /// The objects other than `loco`, in id order: `(template, producer, hitched, position)`.
    fn others(g: &GameLogic, loco: ObjectId) -> Vec<(String, ObjectId, bool, Coord3D)> {
        let mut v: Vec<_> = g
            .objects()
            .filter(|o| o.id != loco)
            .map(|o| (o.id, (o.template.name.clone(), o.producer_id, o.railroad_hitched, o.position)))
            .collect();
        v.sort_by_key(|(id, _)| *id);
        v.into_iter().map(|(_, x)| x).collect()
    }

    #[test]
    fn locomotive_creates_its_carriages_in_list_order() {
        let cab = {
            let mut c = car("Cab", rail_car(false, &[]), 5.0);
            c.ai_update = Some(0);
            c
        };
        let mut g = straight_world(
            vec![car("Loco", rail_car(true, &["CarA", "Cab"]), 5.0), car("CarA", rail_car(false, &[]), 5.0), cab],
            false,
        );
        let l = spawn(&mut g, "Loco", Coord3D::new(100.0, 100.0, 0.0), 0.0);
        g.run_frames(2);
        let made = others(&g, l);
        let names: Vec<&str> = made.iter().map(|m| m.0.as_str()).collect();
        assert_eq!(names, ["CarA", "Cab"], "the list is built front to back");
        for (_, producer, hitched, _) in &made {
            assert_eq!(*producer, l, "each carriage is produced by the locomotive");
            assert!(*hitched, "each carriage is on the train");
        }
        assert!(!g.find_object(l).unwrap().railroad_hitched, "the locomotive leads, it is not hitched");
        // The cab's `AIUpdate` is created with it: its own state machines enter AI_IDLE (AIStates.cpp:1327).
        assert!(g.find_object(ObjectId(l.0 + 2)).unwrap().ai.machine.idle_inited, "the cab is an AI object");
    }

    #[test]
    fn no_waypoint_means_no_track_and_no_carriages() {
        let mut g = world(vec![car("Loco", rail_car(true, &["CarA"]), 5.0), car("CarA", rail_car(false, &[]), 5.0)], Vec::new(), &[]);
        let l = spawn(&mut g, "Loco", Coord3D::new(100.0, 100.0, 0.0), 0.0);
        g.run_frames(2);
        assert!(others(&g, l).is_empty(), "loadTrackData finds no anchor, so createCarriages never runs");
        assert!(state(&g, l).track_data_loaded, "loadTrackData is still only tried once");
    }

    #[test]
    fn only_a_locomotive_creates_carriages() {
        let mut g = straight_world(vec![car("Car", rail_car(false, &["CarA"]), 5.0), car("CarA", rail_car(false, &[]), 5.0)], false);
        let c = spawn(&mut g, "Car", Coord3D::new(100.0, 100.0, 0.0), 0.0);
        g.run_frames(2);
        assert!(others(&g, c).is_empty(), "IsLocomotive = No: the first update does not load the track");
        assert!(state(&g, c).track.is_none(), "a non-locomotive never builds a track");
    }

    #[test]
    fn a_missing_template_or_a_carriage_without_railroad_ends_the_chain() {
        // `findTemplate` returns null: the list stops there (RailroadGuideAIUpdate.cpp:1060).
        let mut g = straight_world(
            vec![car("Loco", rail_car(true, &["CarA", "Missing", "Cab"]), 5.0), car("CarA", rail_car(false, &[]), 5.0)],
            false,
        );
        let l = spawn(&mut g, "Loco", Coord3D::new(100.0, 100.0, 0.0), 0.0);
        g.run_frames(2);
        let names: Vec<String> = others(&g, l).into_iter().map(|m| m.0).collect();
        assert_eq!(names, ["CarA"]);
        // A carriage with no `RailroadBehavior` is created, but nothing is hitched behind it (the assertion, :1076).
        let mut g = straight_world(
            vec![car("Loco", rail_car(true, &["Plain", "CarA"]), 5.0), car("Plain", None, 5.0), car("CarA", rail_car(false, &[]), 5.0)],
            false,
        );
        let l = spawn(&mut g, "Loco", Coord3D::new(100.0, 100.0, 0.0), 0.0);
        g.run_frames(2);
        let made = others(&g, l);
        assert_eq!(made.iter().map(|m| m.0.as_str()).collect::<Vec<_>>(), ["Plain"]);
        assert!(!made[0].2, "a carriage without RailroadBehavior is not hitched");
    }

    #[test]
    fn a_free_carriage_in_reach_is_hitched_instead_of_the_list() {
        // The loco at (100,100) faces +x: its hitch point is maxRadius = 2 * 5 = 10 behind it, at (90,100).
        let mut g = straight_world(vec![car("Loco", rail_car(true, &["CarA"]), 5.0), car("CarA", rail_car(false, &[]), 5.0)], false);
        let loco = spawn(&mut g, "Loco", Coord3D::new(100.0, 100.0, 0.0), 0.0);
        let near = spawn(&mut g, "CarA", Coord3D::new(90.0, 100.0, 0.0), 0.0);
        // The next free car sits 9 behind `near`'s own hitch point (near (90) - 10 = 80), so it is in reach of `near`.
        let behind = spawn(&mut g, "CarA", Coord3D::new(79.0, 100.0, 0.0), 0.0);
        g.run_frames(2);
        let made = others(&g, loco);
        assert_eq!(made.len(), 2, "no carriage is created: the list is ignored when a free car is in reach");
        assert_eq!(state(&g, loco).trailer, near, "the car in reach is the loco's trailer");
        assert_eq!(state(&g, near).trailer, behind, "the chain follows to the next free car");
        assert!(state(&g, near).track.is_some(), "a hitched car inherits the locomotive's track");
        let _ = behind;
    }

    #[test]
    fn a_hitched_or_enemy_carriage_is_not_taken() {
        // Another player's car (the default relationship is not allies) is not a carriage for us.
        let mut g = straight_world(vec![car("Loco", rail_car(true, &["CarA"]), 5.0), car("CarA", rail_car(false, &[]), 5.0)], false);
        let loco = spawn(&mut g, "Loco", Coord3D::new(100.0, 100.0, 0.0), 0.0);
        let car_a = g.find_template("CarA").expect("template");
        let foreign = g.create_object(&car_a, Coord3D::new(90.0, 100.0, 0.0), 0.0, 1);
        g.set_enemies(0, 1);
        g.run_frames(2);
        assert_eq!(others(&g, loco).len(), 2, "the list builds the train, the enemy car is ignored");
        assert!(!g.find_object(foreign).unwrap().railroad_hitched, "an enemy car is never hitched");
    }

    /// `createCarriages` runs nothing unless the first `CarriageTemplateName` resolves (RailroadGuideAIUpdate.cpp:980,
    /// `if (iter != end)` and `if (temp)`): an empty list or an unresolved first name hitches no nearby car either.
    #[test]
    fn an_empty_or_unresolved_list_hitches_nothing() {
        for list in [&[][..], &["Missing"][..]] {
            let mut g = straight_world(vec![car("Loco", rail_car(true, list), 5.0), car("CarA", rail_car(false, &[]), 5.0)], false);
            let loco = spawn(&mut g, "Loco", Coord3D::new(100.0, 100.0, 0.0), 0.0);
            // A free ally car exactly where the proximity hitch would take it (9 units behind the loco's hitch point).
            let near = spawn(&mut g, "CarA", Coord3D::new(90.0, 100.0, 0.0), 0.0);
            g.run_frames(2);
            assert!(!g.find_object(near).unwrap().railroad_hitched, "list {list:?}: the nearby car is not taken");
            assert_eq!(others(&g, loco).len(), 1, "list {list:?}: nothing is created");
        }
    }

    /// The ACCELERATE state machine (`:744-762`): `speed += 0.02; speed *= acceleration`, clamped
    /// to `SpeedMax`. Hand-computed from the C++ for the module defaults.
    #[test]
    fn the_conductor_accelerates_like_the_cpp_recurrence() {
        let mut g = straight_world(vec![car("Loco", rail_car(true, &[]), 5.0)], true);
        let loco = spawn(&mut g, "Loco", Coord3D::new(0.0, 0.0, 0.0), 0.0);
        g.run_frames(1); // frame 0: the module's first call is frame 1 (`setWakeFrame(now + 1)`)
        let mut speed = 0.0f32;
        let mut distance = 0.0f32;
        for frame in 1..=6 {
            // `Railroad::default()` acceleration (1.01, `:91`).
            speed = (speed + 0.02) * 1.01;
            distance += speed;
            g.run_frames(1);
            let st = state(&g, loco);
            assert!((st.conductor.speed - speed).abs() < 1e-6, "frame {frame}: speed {} != {speed}", st.conductor.speed);
            assert!(
                (st.conductor.track_distance - distance).abs() < 1e-5,
                "frame {frame}: distance {} != {distance}",
                st.conductor.track_distance
            );
        }
        assert_eq!(state(&g, loco).conductor_state, ConductorState::Accelerate);
    }

    /// The locomotive is placed on the track: its rear hitch point sits on the track at
    /// `conductorPullInfo.trackDistance - 2 * majorRadius`, and it faces along the track.
    #[test]
    fn the_locomotive_is_placed_on_the_track_by_its_rear_hitch_point() {
        let mut g = straight_world(vec![car("Loco", rail_car(true, &[]), 5.0)], true);
        let loco = spawn(&mut g, "Loco", Coord3D::new(0.0, 0.0, 0.0), 0.0);
        g.run_frames(1);
        g.run_frames(40);
        let mut st = state(&g, loco);
        let o = g.find_object(loco).unwrap();
        // The rear hitch point: the object position minus radius along its heading.
        let rear = Coord3D::new(
            o.position.x - cos(o.angle) * 5.0,
            o.position.y - sin(o.angle) * 5.0,
            o.position.z,
        );
        let expected = st.find_pos_by_path_distance(st.pull.track_distance, st.track.as_ref().unwrap().length, false);
        assert!(
            (rear.x - expected.x).abs() < 0.01 && (rear.y - expected.y).abs() < 0.01,
            "rear {rear:?} should be the track point {expected:?} at the pull distance"
        );
        // The locomotive's own pull distance is `2 * majorRadius` behind the conductor's.
        assert!((st.conductor.track_distance - st.pull.track_distance - 10.0).abs() < 1e-4);
    }

    /// The carriages ride the same track: each car's track distance is its puller's minus
    /// `2 * own majorRadius` (`updatePositionTrackDistance`, :1274), so a longer car leaves a
    /// bigger gap.
    #[test]
    fn carriages_follow_at_twice_their_own_radius() {
        let mut g = straight_world(
            vec![
                car("Loco", rail_car(true, &["Big", "Small"]), 5.0),
                car("Big", rail_car(false, &[]), 30.0),
                car("Small", rail_car(false, &[]), 10.0),
            ],
            true,
        );
        let loco = spawn(&mut g, "Loco", Coord3D::new(0.0, 0.0, 0.0), 0.0);
        g.run_frames(1);
        g.run_frames(40);
        let (big, small) = (ObjectId(loco.0 + 1), ObjectId(loco.0 + 2));
        let (sl, sb, ss) = (state(&g, loco), state(&g, big), state(&g, small));
        assert!((sb.pull.track_distance - (sl.pull.track_distance - 60.0)).abs() < 1e-3, "Big is 60 behind the loco");
        assert!((ss.pull.track_distance - (sb.pull.track_distance - 20.0)).abs() < 1e-3, "Small is 20 behind Big");
        assert_eq!(sb.trailer, small, "the chain is loco -> Big -> Small");
        assert_eq!(ss.trailer, INVALID_ID, "the last car is the caboose");
    }

    /// A non-looping track: a car behind the start waits in the wings at the anchor (hidden), and
    /// the locomotive reaches the end of the line (`m_endOfLine`).
    ///
    /// Two carriages: a train whose *first* car is also its last (the caboose) is a rear-end crash
    /// with itself — the locomotive's `onCollide` kills it (`isRailroad` is true for the last car,
    /// `RailroadGuideAIUpdate.cpp:199-200`, `:236-239`).
    #[test]
    fn a_non_looping_track_hides_the_waiting_cars_and_ends_at_the_line() {
        let mut g = straight_world(
            vec![
                car("Loco", rail_car(true, &["Car", "Car"]), 5.0),
                car("Car", rail_car(false, &[]), 5.0),
            ],
            false,
        );
        let loco = spawn(&mut g, "Loco", Coord3D::new(0.0, 0.0, 0.0), 0.0);
        let c = ObjectId(loco.0 + 1);
        // Frame 0 (nothing runs), frame 1 (the loco creates and pulls the car), frame 2 (the car's
        // own update hides it).
        g.run_frames(3);
        assert!(state(&g, c).waiting_in_wings, "the car's pull distance is negative, so it waits at the anchor");
        assert!(g.find_object(c).unwrap().is_drawable_hidden(), "a car in the wings is not drawn on a straight track");
        // 200 units at up to 4 per frame: stop on the frame the locomotive crosses the end (it is
        // destroyed on a later frame, when its trailer is gone and it is still past the line).
        let mut ended = false;
        for _ in 0..500 {
            g.run_frames(1);
            if g.railroad_states.get(&loco).is_some_and(|s| s.end_of_line) {
                ended = true;
                break;
            }
        }
        assert!(ended, "the locomotive passed the end of the line");
        assert!(g.find_object(loco).unwrap().is_drawable_hidden(), "the end-of-line locomotive is hidden too");
    }

    /// A looping track never hides a car: `setDrawableHidden` is under `if (!m_isLooping)`
    /// (`:834-837`), and a negative distance wraps instead of waiting in the wings. Two carriages
    /// on a full-size loop: a one-car train is its own rear-end crash (the last car is an end), and
    /// the short `straight_world` seam would make the train run into its own tail.
    #[test]
    fn a_looping_track_never_hides_the_carriages() {
        let mut g = square_loop(vec![
            car("Loco", rail_car(true, &["Car", "Car"]), 5.0),
            car("Car", rail_car(false, &[]), 5.0),
        ]);
        let loco = spawn(&mut g, "Loco", Coord3D::new(0.0, 0.0, 0.0), 0.0);
        let c = ObjectId(loco.0 + 1);
        g.run_frames(3);
        g.run_frames(40);
        assert!(!state(&g, c).waiting_in_wings, "a looping track wraps a negative distance");
        assert!(!g.find_object(c).unwrap().is_drawable_hidden(), "a looping track never calls setDrawableHidden");
        assert!(!state(&g, loco).end_of_line, "a looping track never ends");
    }

    /// `m_inTunnel` comes from the waypoint the car is passing and stops the z snap to the ground
    /// (`:1334-1335`); the track's tunnel points are the waypoints whose name ends with `Tunnel`.
    /// On a sloping terrain the frozen z is distinguishable from the ground under the car.
    #[test]
    fn a_tunnel_waypoint_stops_the_z_snap() {
        // Ground rises 0.625 per 10 world units (sample x -> x * MAP_HEIGHT_SCALE).
        let mut data = vec![0u8; 42 * 42];
        for y in 0..42 {
            for x in 0..42 {
                data[y * 42 + x] = x as u8;
            }
        }
        let terrain = TerrainLogic::from_samples(42, 42, 0, data);
        // The tunnel segment is the 100..1000 arc (the `HillTunnel` waypoint's point starts it); the
        // train leaves it at the `Flat` waypoint. The track is long enough that the locomotive does
        // not reach the end of the line within the test (a trailer-less locomotive past the line
        // destroys itself, `:820-821`).
        let wps = vec![
            (1, "Start", Coord3D::new(0.0, 0.0, 0.0)),
            (2, "HillTunnel", Coord3D::new(100.0, 0.0, 0.0)),
            (3, "Flat", Coord3D::new(1000.0, 0.0, 0.0)),
            (4, "End", Coord3D::new(1100.0, 0.0, 0.0)),
        ];
        let mut g = world_on(terrain, vec![car("Loco", rail_car(true, &[]), 5.0)], wps, &[(1, 2), (2, 3), (3, 4)]);
        let loco = spawn(&mut g, "Loco", Coord3D::new(0.0, 0.0, 0.0), 0.0);
        g.run_frames(1);

        // Drive until the car's rear is inside the tunnel segment (arc 100..200).
        let mut z_in_tunnel = None;
        for _ in 0..200 {
            g.run_frames(1);
            if state(&g, loco).in_tunnel {
                z_in_tunnel = Some(g.find_object(loco).unwrap().position.z);
                break;
            }
        }
        let z_in_tunnel = z_in_tunnel.expect("the train passes through the tunnel waypoint");

        // Still in the tunnel 60 frames later: the ground under the car has risen, but the z is
        // the value it had when it entered.
        g.run_frames(60);
        let st = state(&g, loco);
        assert!(st.in_tunnel, "still in the tunnel segment");
        let o = g.find_object(loco).unwrap();
        let turn_x = o.position.x - cos(o.angle) * 5.0;
        let turn_y = o.position.y - sin(o.angle) * 5.0;
        let ground = g.terrain().ground_height(turn_x, turn_y);
        assert!(ground > z_in_tunnel + 3.0, "the ground rose under the train: {ground} vs {z_in_tunnel}");
        assert!((o.position.z - z_in_tunnel).abs() < 0.001, "the z is frozen in a tunnel: {} vs {z_in_tunnel}", o.position.z);

        // Once out of the tunnel the z snaps to the ground again.
        let mut left = false;
        for _ in 0..400 {
            g.run_frames(1);
            if !state(&g, loco).in_tunnel {
                left = true;
                break;
            }
        }
        assert!(left, "the train leaves the tunnel segment");
        let o = g.find_object(loco).unwrap();
        let turn_x = o.position.x - cos(o.angle) * 5.0;
        let turn_y = o.position.y - sin(o.angle) * 5.0;
        let ground = g.terrain().ground_height(turn_x, turn_y);
        assert!((o.position.z - ground).abs() < 0.001, "outside a tunnel the z is the ground height: {} vs {ground}", o.position.z);
    }

    /// `APPLY_BRAKES` -> `WAIT_AT_STATION` -> `ACCELERATE` (`:702-743`): a locomotive that crosses a
    /// `Station` waypoint brakes to a stop, waits `WaitAtStationTime` frames and starts again at
    /// `0.05 * direction`.
    #[test]
    fn a_station_waypoint_brakes_waits_and_restarts() {
        // The station segment is the 200..2200 arc: `loadTrackData` stores the *from* waypoint as a
        // point's handle, so the first two points share the anchor's handle and the point after the
        // anchor can never be an edge (`m_currentPointHandle != handle`, :1444). The station must
        // therefore sit on the third waypoint to be smelled at all.
        let wps = vec![
            (1, "Start", Coord3D::new(0.0, 0.0, 0.0)),
            (2, "Mid", Coord3D::new(100.0, 0.0, 0.0)),
            (3, "DepotStation", Coord3D::new(200.0, 0.0, 0.0)),
            (4, "End", Coord3D::new(2200.0, 0.0, 0.0)),
        ];
        let mut rail = rail_car(true, &[]).unwrap();
        rail.wait_at_station_time = 10;
        let mut g = world(vec![car("Loco", Some(rail), 5.0)], wps, &[(1, 2), (2, 3), (3, 4)]);
        let loco = spawn(&mut g, "Loco", Coord3D::new(0.0, 0.0, 0.0), 0.0);
        g.run_frames(1);
        // The station's `distanceFromFirst` is 100; the car's own point is the segment's *first*
        // point, so drive until the conductor state leaves ACCELERATE.
        let mut braked = false;
        for _ in 0..400 {
            g.run_frames(1);
            if state(&g, loco).conductor_state == ConductorState::ApplyBrakes {
                braked = true;
                break;
            }
        }
        assert!(braked, "the locomotive smells the station and applies the brakes");
        // Brake to a stop (0.99 per frame from ~4 takes ~370 frames), then wait exactly
        // `WaitAtStationTime` frames and restart.
        let mut stopped = false;
        for _ in 0..600 {
            g.run_frames(1);
            let st = state(&g, loco);
            if st.conductor_state == ConductorState::WaitAtStation {
                assert_eq!(st.conductor.speed, 0.0);
                assert_eq!(st.wait_at_station_timer, 10);
                stopped = true;
                break;
            }
        }
        assert!(stopped, "the brakes bring the conductor to a stop");
        for _ in 0..9 {
            g.run_frames(1);
            assert_eq!(state(&g, loco).conductor_state, ConductorState::WaitAtStation, "the timer has not expired");
        }
        g.run_frames(1);
        let st = state(&g, loco);
        assert_eq!(st.conductor_state, ConductorState::Accelerate, "the timer expired");
        assert!((st.conductor.speed - 0.05).abs() < 1e-6, "the restart speed is 0.05 * direction");
    }

    /// A vehicle with a `PhysicsBehavior` module: something the train can trample, crush or shove.
    fn victim_template(name: &str, radius: f32, health: f32) -> ObjectTemplate {
        let mut t = ObjectTemplate::new(name);
        t.kind_of = kind_of::VEHICLE;
        t.geometry.major_radius = radius;
        t.max_health = health;
        t.initial_health = health;
        t.physics = Some(crate::physics::PhysicsTemplate::default());
        t
    }

    /// Put a car into a running state without waiting for the acceleration recurrence, and clear the
    /// "not on the track yet" flag `onCollide` returns on.
    fn run_train(g: &mut GameLogic, id: ObjectId, speed: f32) {
        let st = g.railroad_states.get_mut(&id).expect("a railroad object");
        st.waiting_in_wings = false;
        st.end_of_line = false;
        st.conductor_state = ConductorState::Accelerate;
        st.conductor.speed = speed;
        st.pull.speed = speed;
    }

    /// Make `id` the end of a stranded train: it has a track, has left the wings and is either the
    /// lead carriage or the last car (`isRailroad`, `RailroadGuideAIUpdate.cpp:188-205`).
    fn strand(g: &mut GameLogic, id: ObjectId, lead_carriage: bool) {
        let st = g.railroad_states.get_mut(&id).expect("a railroad object");
        st.track = Some(Arc::new(TrainTrack { points: Vec::new(), is_looping: true, length: 100.0 }));
        st.waiting_in_wings = false;
        st.end_of_line = false;
        st.is_lead_carriage = lead_carriage;
        // It keeps a car behind it (`m_trailerID`), so only a lead carriage or the last car (a
        // trailer of INVALID_ID) counts as an end.
        st.trailer = ObjectId(9999);
    }

    /// `onCollide` (`RailroadGuideAIUpdate.cpp:366-398`): a locomotive at or above `KillSpeedMin`
    /// kills what it hits, draws the victim's pitch and roll rates in that order
    /// (`GameLogicRandomValueReal(-0.03, 0.03)` twice), lifts it off the ground and throws it with
    /// `GameLogicRandomValueReal(0.05, speed/10)` in z.
    #[test]
    fn a_fast_locomotive_tramples_an_overlapping_vehicle() {
        let mut g = straight_world(vec![car("Loco", rail_car(true, &[]), 5.0), victim_template("Tank", 5.0, 100.0)], true);
        let loco = spawn(&mut g, "Loco", Coord3D::new(100.0, 0.0, 0.0), 0.0);
        let tank = spawn(&mut g, "Tank", Coord3D::new(102.0, 0.0, 0.0), 0.0);
        run_train(&mut g, loco, 3.0);
        // The draws the C++ makes, in order: pitch, roll, then the shove's z.
        let mut expected = g.rng.clone();
        let pitch = expected.value_real(-0.03, 0.03);
        let roll = expected.value_real(-0.03, 0.03);
        let lift = expected.value_real(0.05, 0.3);
        g.process_object_collisions();
        assert_eq!(g.rng, expected, "exactly the pitch, roll and heft draws, in that order");
        // `kill_object` runs the die path (a template with no die modules is destroyed, as the
        // engine's `DefaultThingTemplate` DestroyDie would), so `find_object` no longer lists it.
        let o = g.objects.get(&tank).expect("the object stays until process_destroy_list");
        assert!(o.destroyed || o.is_dying(), "3 >= KillSpeedMin 1 kills");
        // `(theirLoc - myLoc)` is 2 long, us+them = 10: overlap = ((10-2)+1)/4 = 2.25 forward.
        assert_eq!(o.position, Coord3D::new(104.25, 0.0, 2.0), "shoved out of the way and lifted to ground+2");
        let p = o.physics.as_ref().unwrap();
        assert_eq!((p.pitch_rate, p.roll_rate), (pitch, roll), "the two trample draws land on pitch then roll");
        // delta = unit (1,0,0) * min(1.4, 3*0.66) = 1.4 in x; dot = 1; z = the heft draw * dot.
        assert_eq!(p.velocity, Coord3D::new(1.4, 0.0, lift), "thrown with the fake heft");
        assert!(p.allow_to_fall && p.allow_bounce && p.apply_friction_2d_when_airborne, "setAllowToFall/Bouncing/AirborneFriction");
        assert_eq!(p.yaw_rate, 0.0, "a head-on hit does not spin the victim");
    }

    /// `PartitionData` only exists for an object with at least one grid cell: the engine's
    /// `updateCellsTouched` -> `doRectFill` finds no cell outside the map extents, so
    /// `addPossibleCollisions` adds nothing and the object never collides there
    /// (`PartitionManager.cpp:2058-2135`, `:2796-2820`). An off-map train therefore runs through
    /// anything, which is what AN3's Alpine train does while its track is at `x < 0`.
    #[test]
    fn an_off_map_train_has_no_collisions() {
        let mut g = straight_world(vec![car("Loco", rail_car(true, &[]), 5.0), victim_template("Tank", 5.0, 100.0)], true);
        // The 400x400 world's partition covers x,y in [0,400): keep the pair off the map.
        let loco = spawn(&mut g, "Loco", Coord3D::new(-100.0, -100.0, 0.0), 0.0);
        let tank = spawn(&mut g, "Tank", Coord3D::new(-98.0, -100.0, 0.0), 0.0);
        run_train(&mut g, loco, 3.0);
        let before = g.rng.clone();
        g.process_object_collisions();
        let o = g.objects.get(&tank).unwrap();
        assert!(!o.destroyed && !o.is_dying(), "no partition cells, no contact");
        assert_eq!(o.position, Coord3D::new(-98.0, -100.0, 0.0), "not shoved either");
        assert_eq!(g.rng, before, "no trample draws");
        // The same pair on the map collides (the control: the trample test above).
    }

    /// Below `KillSpeedMin` the same hit is a `DAMAGE_CRUSH` of `speed * 10` instead of a kill
    /// (`:375-385`), with no random draw: at speed 0.5 the heft range is `[0.05, 0.05]`, which
    /// returns without advancing the generator (the `delta <= 0` branch of
    /// `GetGameLogicRandomValueReal`, `RandomValue.cpp:294-298`).
    #[test]
    fn a_slow_locomotive_crushes_instead_of_killing() {
        let mut g = straight_world(vec![car("Loco", rail_car(true, &[]), 5.0), victim_template("Tank", 5.0, 100.0)], true);
        let loco = spawn(&mut g, "Loco", Coord3D::new(100.0, 0.0, 0.0), 0.0);
        let tank = spawn(&mut g, "Tank", Coord3D::new(102.0, 0.0, 0.0), 0.0);
        run_train(&mut g, loco, 0.5);
        let before = g.rng.clone();
        g.process_object_collisions();
        assert_eq!(g.rng, before, "0.05..0.05 returns hi without a draw");
        let o = g.find_object(tank).unwrap();
        assert!(!o.is_dying(), "0.5 < KillSpeedMin 1: it survives the hit");
        assert_eq!(o.health(), 95.0, "DAMAGE_CRUSH for speed * 10");
        assert_eq!(o.last_damage_info.kind, crate::damage::DamageType::Crush);
        assert_eq!(o.last_damage_info.source, loco);
        assert_eq!(o.position, Coord3D::new(104.25, 0.0, 2.0));
        assert_eq!(o.physics.as_ref().unwrap().velocity, Coord3D::new(0.33, 0.0, 0.05));
    }

    /// `:405-415`: the victim's yaw rate is `deviationCOG * -0.06 * speed`, where `deviationCOG` is
    /// the victim direction against the train's left/right axis.
    #[test]
    fn a_side_hit_spins_the_victim() {
        let mut g = straight_world(vec![car("Loco", rail_car(true, &[]), 5.0), victim_template("Tank", 5.0, 100.0)], true);
        let loco = spawn(&mut g, "Loco", Coord3D::new(100.0, 0.0, 0.0), 0.0);
        // Two units to the side: the victim is shoved forward+right and the train's +x facing sees it
        // on its right, so the yaw rate is positive.
        let tank = spawn(&mut g, "Tank", Coord3D::new(102.0, 2.0, 0.0), 0.0);
        run_train(&mut g, loco, 2.0);
        g.process_object_collisions();
        let o = g.objects.get(&tank).expect("the object stays until process_destroy_list");
        let p = o.physics.as_ref().unwrap();
        assert!(p.yaw_rate > 0.0, "spins the victim away: {}", p.yaw_rate);
        assert!(o.position.x > 102.0 && o.position.y > 2.0, "shoved along the hit normal: {:?}", o.position);
    }

    /// `:396-398`: a thrown infantryman that already flies faster than 5 units/frame is not thrown
    /// again (but is still hit and lifted).
    #[test]
    fn a_flying_infantry_is_not_thrown_again() {
        let mut inf = victim_template("Rifle", 2.0, 100.0);
        inf.kind_of |= kind_of::INFANTRY;
        let mut g = straight_world(vec![car("Loco", rail_car(true, &[]), 5.0), inf], true);
        let loco = spawn(&mut g, "Loco", Coord3D::new(100.0, 0.0, 0.0), 0.0);
        let rifle = spawn(&mut g, "Rifle", Coord3D::new(102.0, 0.0, 0.0), 0.0);
        if let Some(p) = g.objects.get_mut(&rifle).and_then(|o| o.physics.as_mut()) {
            p.velocity = Coord3D::new(0.0, 0.0, 6.0);
        }
        run_train(&mut g, loco, 3.0);
        g.process_object_collisions();
        let o = g.objects.get(&rifle).expect("the object stays until process_destroy_list");
        assert!(o.destroyed || o.is_dying(), "still trampled");
        assert_eq!(o.physics.as_ref().unwrap().velocity, Coord3D::new(0.0, 0.0, 6.0), "not hurled again");
        assert_eq!(o.position.z, 2.0, "but still lifted off the ground");
        // The infantryman is not shoved out of the footprint first (`if (!victimIsInfantry)`, :328).
        assert_eq!((o.position.x, o.position.y), (102.0, 0.0));
    }

    /// `:227-249`: a train car only kills on a rear-end crash when the *other* object is an end of a
    /// train (`isRailroad`). A locomotive that hits a stranded end kills it and survives; a middle car
    /// (not the lead, still pulling a trailer) refuses to cooperate, so a train does not kill itself.
    #[test]
    fn a_train_rear_ending_another_kills_its_end() {
        let mut g = straight_world(vec![car("Loco", rail_car(true, &[]), 5.0), car("Stranded", rail_car(false, &[]), 5.0)], true);
        let loco = spawn(&mut g, "Loco", Coord3D::new(100.0, 0.0, 0.0), 0.0);
        let stranded = spawn(&mut g, "Stranded", Coord3D::new(102.0, 0.0, 0.0), 0.0);
        run_train(&mut g, loco, 3.0);
        strand(&mut g, stranded, true);
        g.process_object_collisions();
        assert!(g.objects[&stranded].destroyed || g.objects[&stranded].is_dying(), "the stranded train's end is killed");
        assert!(!g.objects[&loco].destroyed && !g.objects[&loco].is_dying(), "the locomotive survives a rear-end");

        // A middle car of the other train: `isRailroad` is false, and a train car has no physics, so
        // the collision does nothing at all.
        let mut g = straight_world(vec![car("Loco", rail_car(true, &[]), 5.0), car("Middle", rail_car(false, &[]), 5.0)], true);
        let loco = spawn(&mut g, "Loco", Coord3D::new(100.0, 0.0, 0.0), 0.0);
        let middle = spawn(&mut g, "Middle", Coord3D::new(102.0, 0.0, 0.0), 0.0);
        run_train(&mut g, loco, 3.0);
        strand(&mut g, middle, false);
        g.process_object_collisions();
        assert!(!g.objects[&middle].destroyed && !g.objects[&middle].is_dying(), "a middle car is not an end of the train");
        assert!(!g.objects[&loco].destroyed && !g.objects[&loco].is_dying());

        // A lead carriage that coasts into another train's end kills both (`:240-244`).
        let mut g = straight_world(
            vec![car("Lead", rail_car(false, &[]), 5.0), car("End", rail_car(false, &[]), 5.0)],
            true,
        );
        let lead = spawn(&mut g, "Lead", Coord3D::new(100.0, 0.0, 0.0), 0.0);
        let end = spawn(&mut g, "End", Coord3D::new(102.0, 0.0, 0.0), 0.0);
        run_train(&mut g, lead, 3.0);
        strand(&mut g, lead, true);
        strand(&mut g, end, true);
        g.process_object_collisions();
        assert!(g.objects[&end].destroyed || g.objects[&end].is_dying(), "the other train's end is killed");
        assert!(g.objects[&lead].destroyed || g.objects[&lead].is_dying(), "and the lead carriage dies too");
    }

    /// `makeAWallOutOfThisTrain` (`:640-663`) at `APPLY_BRAKES` -> `WAIT_AT_STATION` (`:713`) and at
    /// the restart (`:733`): the stopped train's footprint becomes a pathfinding obstacle — every car
    /// behind it too — and stops blocking when the locomotive departs.
    #[test]
    fn a_stopped_train_blocks_the_pathfinder_until_it_departs() {
        // The same station-on-the-third-waypoint track as the brake test, on a map large enough for
        // the whole run.
        let wps = vec![
            (1, "Start", Coord3D::new(0.0, 0.0, 0.0)),
            (2, "Mid", Coord3D::new(100.0, 0.0, 0.0)),
            (3, "DepotStation", Coord3D::new(200.0, 0.0, 0.0)),
            (4, "End", Coord3D::new(900.0, 0.0, 0.0)),
        ];
        let mut rail = rail_car(true, &["Car", "Car"]).unwrap();
        rail.wait_at_station_time = 20;
        let mut g = world_on(
            TerrainLogic::flat(1000, 1000, 0),
            vec![car("Loco", Some(rail), 5.0), car("Car", rail_car(false, &[]), 10.0)],
            wps,
            &[(1, 2), (2, 3), (3, 4)],
        );
        let loco = spawn(&mut g, "Loco", Coord3D::new(0.0, 0.0, 0.0), 0.0);
        // Frame 0: nothing; frame 1: the locomotive loads the track and creates its cars.
        g.run_frames(2);
        let car = ObjectId(loco.0 + 1);
        let tail = ObjectId(loco.0 + 2);
        assert!(g.find_object(car).is_some() && g.find_object(tail).is_some(), "the carriages were created");
        let mut stopped = false;
        for _ in 0..2000 {
            g.run_frames(1);
            if state(&g, loco).conductor_state == ConductorState::WaitAtStation {
                stopped = true;
                break;
            }
        }
        assert!(stopped, "the locomotive stops at the station");
        let loco_pos = g.find_object(loco).unwrap().position;
        let car_pos = g.find_object(car).unwrap().position;
        let tail_pos = g.find_object(tail).unwrap().position;
        assert!(loco_pos.dist_2d(&car_pos) > 10.0, "the cars are in different cells: {loco_pos:?} vs {car_pos:?}");
        let obstacle = Some(crate::pathfind::CellType::Obstacle);
        assert_eq!(g.pathfinder.cell_type_at(&loco_pos), obstacle, "the locomotive is a wall");
        assert_eq!(g.pathfinder.cell_type_at(&car_pos), obstacle, "and so is each car behind it");
        assert_eq!(g.pathfinder.cell_type_at(&tail_pos), obstacle);
        // Wait out the station stop; the wall is removed on the restart frame, before the train moves.
        g.run_frames(20);
        assert_eq!(state(&g, loco).conductor_state, ConductorState::Accelerate, "the timer expired");
        let clear = Some(crate::pathfind::CellType::Clear);
        assert_eq!(g.pathfinder.cell_type_at(&loco_pos), clear, "the departing train stops blocking");
        assert_eq!(g.pathfinder.cell_type_at(&car_pos), clear);
        assert_eq!(g.pathfinder.cell_type_at(&tail_pos), clear);
    }

    /// `m_waitAtStationTimer` is an `UnsignedInt` (`RailroadGuideAIUpdate.h:85`): `--t` on 0 wraps and
    /// `t <= 0` is `t == 0` (`:725-726`), so `WaitAtStationTime = 0` leaves the train waiting for
    /// 2^32-1 frames instead of restarting at once.
    #[test]
    fn a_zero_wait_at_station_wraps_and_never_restarts() {
        let wps = vec![
            (1, "Start", Coord3D::new(0.0, 0.0, 0.0)),
            (2, "Mid", Coord3D::new(100.0, 0.0, 0.0)),
            (3, "DepotStation", Coord3D::new(200.0, 0.0, 0.0)),
            (4, "End", Coord3D::new(2200.0, 0.0, 0.0)),
        ];
        let mut rail = rail_car(true, &[]).unwrap();
        rail.wait_at_station_time = 0;
        let mut g = world(vec![car("Loco", Some(rail), 5.0)], wps, &[(1, 2), (2, 3), (3, 4)]);
        let loco = spawn(&mut g, "Loco", Coord3D::new(0.0, 0.0, 0.0), 0.0);
        g.run_frames(1);
        let mut stopped = false;
        for _ in 0..1000 {
            g.run_frames(1);
            if state(&g, loco).conductor_state == ConductorState::WaitAtStation {
                stopped = true;
                break;
            }
        }
        assert!(stopped, "the locomotive stops at the station");
        assert_eq!(state(&g, loco).wait_at_station_timer, 0);
        g.run_frames(50);
        let st = state(&g, loco);
        assert_eq!(st.conductor_state, ConductorState::WaitAtStation, "a zero wait wraps: it never reaches 0 again");
        assert_eq!(st.conductor.speed, 0.0);
        assert!(st.wait_at_station_timer > u32::MAX - 60, "0 - 1 wrapped to {}", st.wait_at_station_timer);
    }
}
