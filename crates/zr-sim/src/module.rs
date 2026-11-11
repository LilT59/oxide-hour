//! Update modules (`UpdateModuleInterface`, GameLogic/Module/UpdateModule.h).

use crate::crc::Crc;
use crate::math::Coord3D;
use crate::object::{Object, ObjectId, PlayerIndex};
use crate::random::GameRandom;
use crate::schedule::UpdateHandle;
use crate::template::ObjectTemplate;
use std::sync::Arc;

/// `UpdateSleepTime`: number of frames until the next call.
pub type UpdateSleepTime = u32;
/// Wake on the next frame (`UPDATE_SLEEP_NONE`).
pub const UPDATE_SLEEP_NONE: UpdateSleepTime = 1;
/// Never wake until explicitly awakened (`UPDATE_SLEEP_FOREVER`).
pub const UPDATE_SLEEP_FOREVER: UpdateSleepTime = 0x3fff_ffff;

/// `SleepyUpdatePhase`: the low two bits of the scheduler priority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SleepyUpdatePhase {
    Initial = 0,
    Physics = 1,
    Normal = 2,
    Final = 3,
}

/// Side effects an update may request. They are applied by `GameLogic` right after the module
/// returns, which matches the original where creation/awakening inside an update takes effect
/// immediately but never re-enters the running module.
#[derive(Debug)]
pub enum Request {
    /// `TheGameLogic->destroyObject`.
    Destroy(ObjectId),
    /// `ThingFactory::newObject`.
    Spawn { template: Arc<ObjectTemplate>, pos: Coord3D, angle: f32, player: PlayerIndex },
    /// `friend_awakenUpdateModule` on another module.
    Wake { handle: UpdateHandle, frame: u32 },
    /// `Pathfinder::queueForPath` (`AIPathfind.cpp:5761`): the object's AI requested a path, so the
    /// object joins the pathfind queue and is serviced by `processPathfindQueue` later this frame.
    QueuePath(ObjectId),
    /// `RailroadBehavior::update`'s first call on a locomotive (`loadTrackData` + `createCarriages`)
    /// and every later call (`RailroadGuideAIUpdate.cpp:670`): the state and the pull chain cross
    /// objects, so `GameLogic` runs it (`railroad.rs`).
    RailroadUpdate(ObjectId),
}

/// Everything an update may touch. The module itself is detached from the scheduler while it runs.
pub struct UpdateContext<'a> {
    /// Current logic frame (`TheGameLogic->getFrame()`).
    pub frame: u32,
    pub object: &'a mut Object,
    pub terrain: &'a crate::terrain::TerrainLogic,
    pub rng: &'a mut GameRandom,
    pub requests: &'a mut Vec<Request>,
}

/// A sleepy update module. `update` returns how many frames to sleep (`>= 1`).
pub trait UpdateModule {
    fn update(&mut self, ctx: &mut UpdateContext) -> UpdateSleepTime;
    /// `getUpdatePhase`.
    fn phase(&self) -> SleepyUpdatePhase {
        SleepyUpdatePhase::Normal
    }
    /// Initial wake frame; `0` means "as soon as registered" (the original's default).
    fn initial_wake_frame(&self) -> u32 {
        0
    }
    /// Hash any module-private state (the object-level state is hashed by `Object`).
    fn crc(&self, _c: &mut Crc) {}
    fn name(&self) -> &'static str;
}
