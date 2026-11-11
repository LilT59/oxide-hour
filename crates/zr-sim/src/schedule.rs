//! The sleepy-update scheduler: a binary max-"lowest priority" heap exactly as in
//! `GameLogic::m_sleepyUpdates` (GameLogic.cpp: `pushSleepyUpdate`, `popSleepyUpdate`,
//! `eraseSleepyUpdate`, `rebalance{Parent,Child}SleepyUpdate`, `friend_awakenUpdateModule`).
//!
//! Priority is `(next_call_frame << 2) | phase` (lower value = earlier = higher priority). The
//! heap is not stable, but its layout is a pure function of the push/pop/erase sequence, so
//! iteration order is deterministic and mirrors the original for identical sequences.
//! Slots live in a slab with a LIFO free list (handles are reused deterministically; a handle is
//! only ever held by its object and the heap, both of which drop it on destroy).

use crate::module::{SleepyUpdatePhase, UpdateModule, UPDATE_SLEEP_FOREVER};
use crate::object::ObjectId;
use crate::sleepy_families::FamilyKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct UpdateHandle(pub u32);

pub(crate) struct UpdateSlot {
    pub object: ObjectId,
    /// `None` while the module is executing (taken out by the game loop) or when the slot is free.
    pub module: Option<Box<dyn UpdateModule>>,
    /// `m_nextCallFrameAndPhase`.
    pub next_call_and_phase: u32,
    pub phase: SleepyUpdatePhase,
    /// `m_indexInLogic`, -1 when not in the heap.
    pub index: i32,
    pub in_use: bool,
    /// The object's `AIUpdateInterface` module. It runs the object's AI state machine (not a
    /// `UpdateModule::update`), from `GameLogic::run_ai_heap_pass`.
    pub ai: bool,
    /// A family slot (`sleepy_families.rs`): its update needs the whole `GameLogic`, so it has no
    /// boxed `module`; `GameLogic::run_sleepy_updates_with` calls `run_family` for it.
    pub family: Option<FamilyKind>,
    /// The object's `SubdualDamageHelper` (object_helpers.rs): its heal goes through the body, so
    /// `GameLogic`'s sleepy loop calls `on_subdual_change` after its update (ActiveBody.cpp:1285).
    pub subdual_heal: bool,
    /// `getDisabledTypesToProcess()`: the disabled flags for which the update runs while the object is disabled
    /// (GameLogic.cpp:3851-3885). An AI slot carries `AIUpdateInterface`'s `DISABLED_HELD` mask (AIUpdate.h:305);
    /// families and helpers set theirs at registration. Default: the base `DISABLEDMASK_NONE` (UpdateModule.h:172).
    pub types_to_process: u16,
}

#[derive(Default)]
pub struct UpdateSchedule {
    slots: Vec<UpdateSlot>,
    free: Vec<u32>,
    heap: Vec<u32>,
    /// `m_curUpdateModule`: the slot whose update is running. A wake for it is ignored
    /// (`GameLogic::friend_awakenUpdateModule`, GameLogic.cpp:3094-3098).
    running: Option<u32>,
}

impl UpdateSchedule {
    pub fn len(&self) -> usize {
        self.heap.len()
    }
    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    /// Allocate a slot for `module` and push it (`registerObject`'s per-module step).
    /// `now` is already clamped to >= 1 by the caller.
    pub(crate) fn register(&mut self, object: ObjectId, module: Box<dyn UpdateModule>, now: u32) -> UpdateHandle {
        let phase = module.phase();
        self.register_with(object, module, now, phase, false)
    }

    /// Register an object's `AIUpdateInterface`. `AIUpdateInterface::getUpdatePhase` is
    /// `PHASE_INITIAL` (AIUpdate.h:607), so every AI update runs before the physics and normal
    /// phases of the same frame. Its `getDisabledTypesToProcess` is `MAKE_DISABLED_MASK(DISABLED_HELD)`
    /// (AIUpdate.h:305): a unit disabled by any other type is deferred by the sleepy loop's gate
    /// (GameLogic.cpp:3849-3885), which is why a contained (HELD) unit keeps running its AI.
    pub(crate) fn register_ai(&mut self, object: ObjectId, module: Box<dyn UpdateModule>, now: u32) -> UpdateHandle {
        self.register_with(object, module, now, SleepyUpdatePhase::Initial, true)
    }

    fn register_with(
        &mut self,
        object: ObjectId,
        module: Box<dyn UpdateModule>,
        now: u32,
        phase: SleepyUpdatePhase,
        ai: bool,
    ) -> UpdateHandle {
        let when = module.initial_wake_frame();
        let when = if when == 0 { now } else { when };
        let slot = UpdateSlot {
            object,
            module: Some(module),
            next_call_and_phase: 0,
            phase,
            index: -1,
            in_use: true,
            ai,
            family: None,
            subdual_heal: false,
            types_to_process: if ai { crate::disabled::DisabledType::Held.bit() } else { 0 },
        };
        self.push_new(slot, when)
    }

    /// Register a family slot (`sleepy_families.rs`) at `PHASE_NORMAL` with its first wake frame `wake`
    /// (`0` = `now`, the `registerObject` rule of GameLogic.cpp:4044-4045).
    pub(crate) fn register_family(&mut self, object: ObjectId, kind: FamilyKind, wake: u32, now: u32) -> UpdateHandle {
        let when = if wake == 0 { now } else { wake };
        let slot = UpdateSlot {
            object,
            module: None,
            next_call_and_phase: 0,
            phase: kind.phase(),
            index: -1,
            in_use: true,
            ai: false,
            family: Some(kind),
            subdual_heal: false,
            types_to_process: kind.types_to_process(),
        };
        self.push_new(slot, when)
    }

    /// Store `slot` in a free or new slot and push it with wake frame `when`.
    fn push_new(&mut self, slot: UpdateSlot, when: u32) -> UpdateHandle {
        let h = if let Some(h) = self.free.pop() {
            self.slots[h as usize] = slot;
            h
        } else {
            self.slots.push(slot);
            (self.slots.len() - 1) as u32
        };
        self.set_next_call_frame(h, when);
        self.push(h);
        UpdateHandle(h)
    }

    /// The heap slots of `object`, in heap-array order: the scan that `GameLogic::destroyObject`'s erase makes
    /// (GameLogic.cpp:2578-2590, `for it2 in m_sleepyUpdates`).
    pub(crate) fn handles_of(&self, object: ObjectId) -> Vec<UpdateHandle> {
        self.heap.iter().filter(|&&h| self.slots[h as usize].object == object).map(|&h| UpdateHandle(h)).collect()
    }

    /// `GameLogic::processDestroyList`'s erase of one object (GameLogic.cpp:2578-2609): its updates are collected in
    /// heap-array order, then erased last to first (each erase may move the others, so the index is re-read).
    pub(crate) fn erase_object(&mut self, object: ObjectId) {
        for h in self.handles_of(object).into_iter().rev() {
            self.erase(h);
        }
    }

    /// `friend_setNextCallFrame`.
    pub(crate) fn set_next_call_frame(&mut self, h: u32, frame: u32) {
        let frame = frame.min(UPDATE_SLEEP_FOREVER);
        let s = &mut self.slots[h as usize];
        s.next_call_and_phase = (frame << 2) | s.phase as u32;
    }

    pub fn next_call_frame(&self, h: UpdateHandle) -> u32 {
        self.slots[h.0 as usize].next_call_and_phase >> 2
    }

    pub(crate) fn slot(&self, h: u32) -> &UpdateSlot {
        &self.slots[h as usize]
    }
    pub(crate) fn slot_mut(&mut self, h: u32) -> &mut UpdateSlot {
        &mut self.slots[h as usize]
    }

    fn pri(&self, h: u32) -> u32 {
        self.slots[h as usize].next_call_and_phase
    }

    /// `isLowerPriority(a, b)`: true iff `a` is later than `b`.
    fn is_lower_priority(&self, a: u32, b: u32) -> bool {
        self.pri(a) > self.pri(b)
    }

    fn swap_in_heap(&mut self, i: usize, j: usize) {
        self.heap.swap(i, j);
        let (hi, hj) = (self.heap[i], self.heap[j]);
        self.slots[hi as usize].index = i as i32;
        self.slots[hj as usize].index = j as i32;
    }

    fn rebalance_parent(&mut self, mut i: usize) -> usize {
        // parent = ((i+1)>>1)-1, computed signed.
        let mut parent = ((i as i64 + 1) >> 1) - 1;
        while parent >= 0 && self.is_lower_priority(self.heap[parent as usize], self.heap[i]) {
            self.swap_in_heap(parent as usize, i);
            i = parent as usize;
            parent = ((parent + 1) >> 1) - 1;
        }
        i
    }

    fn rebalance_child(&mut self, mut i: usize) -> usize {
        let sz = self.heap.len();
        let mut child = (i << 1) + 1;
        while child < sz {
            // choose the higher-priority of the two children
            if child + 1 < sz && self.is_lower_priority(self.heap[child], self.heap[child + 1]) {
                child += 1;
            }
            if !self.is_lower_priority(self.heap[i], self.heap[child]) {
                break;
            }
            self.swap_in_heap(i, child);
            i = child;
            child = (i << 1) + 1;
        }
        i
    }

    /// `rebalanceSleepyUpdate`.
    pub(crate) fn rebalance(&mut self, i: usize) {
        let i = self.rebalance_parent(i);
        self.rebalance_child(i);
    }

    fn push(&mut self, h: u32) {
        self.heap.push(h);
        let i = self.heap.len() - 1;
        self.slots[h as usize].index = i as i32;
        self.rebalance_parent(i);
    }

    /// The heap's slot handles in array order (the `ZR_L99_HEAP` diagnostic dump reads it).
    pub(crate) fn heap_order(&self) -> &[u32] {
        &self.heap
    }

    /// `peekSleepyUpdate`.
    pub(crate) fn peek(&self) -> Option<u32> {
        self.heap.first().copied()
    }

    /// `m_curUpdateModule` for the module that is running now (`None` between updates).
    pub(crate) fn set_running(&mut self, h: Option<u32>) {
        self.running = h;
    }

    /// Whether slot `h` is still an object's live AI module at heap index 0 (it may have been
    /// erased by its own update: a destroyed object).
    pub(crate) fn is_live_at_top(&self, h: u32) -> bool {
        let s = &self.slots[h as usize];
        s.in_use && s.index == 0 && self.heap.first() == Some(&h)
    }

    /// `eraseSleepyUpdate(i)`, also frees the slot (the original deletes the module with its object).
    pub(crate) fn erase(&mut self, h: UpdateHandle) {
        let idx = self.slots[h.0 as usize].index;
        if idx < 0 {
            return;
        }
        let i = idx as usize;
        self.slots[h.0 as usize].index = -1;
        let last = self.heap.len() - 1;
        if i != last {
            self.heap[i] = self.heap[last];
            let moved = self.heap[i];
            self.slots[moved as usize].index = i as i32;
            self.heap.pop();
            self.rebalance(i);
        } else {
            self.heap.pop();
        }
        let s = &mut self.slots[h.0 as usize];
        s.module = None;
        s.in_use = false;
        self.free.push(h.0);
    }

    /// `friend_awakenUpdateModule` for a module that is *not* currently running.
    /// `now` is the current frame; `when` the requested wake frame.
    pub fn awaken(&mut self, h: UpdateHandle, now: u32, when: u32) {
        if self.running == Some(h.0) {
            return; // GameLogic.cpp:3094-3098: a module does not wake itself from its own update
        }
        let s = &self.slots[h.0 as usize];
        if !s.in_use || s.index < 0 {
            return;
        }
        if when == s.next_call_and_phase >> 2 {
            return;
        }
        // Already awake this frame and asked to run "as soon as possible": keep the earlier slot.
        if now > 0 && (s.next_call_and_phase >> 2) == now && when == now + 1 {
            return;
        }
        let idx = s.index as usize;
        self.set_next_call_frame(h.0, when);
        self.rebalance(idx);
    }

    /// Heap-ordered snapshot of `(object, next_call_and_phase)` for hashing.
    pub(crate) fn heap_iter(&self) -> impl Iterator<Item = (ObjectId, u32)> + '_ {
        self.heap.iter().map(|&h| {
            let s = &self.slots[h as usize];
            (s.object, s.next_call_and_phase)
        })
    }

    #[cfg(test)]
    pub(crate) fn validate(&self) {
        for (i, &h) in self.heap.iter().enumerate() {
            assert_eq!(self.slots[h as usize].index, i as i32);
            if i > 0 {
                let p = self.heap[((i + 1) >> 1) - 1];
                assert!(self.pri(p) <= self.pri(h), "heap order violated at {i}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::{UpdateContext, UpdateSleepTime};

    struct Dummy(u32);
    impl UpdateModule for Dummy {
        fn update(&mut self, _: &mut UpdateContext) -> UpdateSleepTime {
            1
        }
        fn initial_wake_frame(&self) -> u32 {
            self.0
        }
        fn name(&self) -> &'static str {
            "dummy"
        }
    }

    #[test]
    fn heap_orders_by_wake_frame_and_survives_erase() {
        let mut s = UpdateSchedule::default();
        let mut hs = Vec::new();
        for (i, w) in [9u32, 3, 7, 1, 8, 2, 6, 5, 4, 10].iter().enumerate() {
            hs.push(s.register(ObjectId(i as u32 + 1), Box::new(Dummy(*w)), 1));
            s.validate();
        }
        s.erase(hs[3]); // wake frame 1
        s.erase(hs[0]); // wake frame 9
        s.validate();
        let mut order = Vec::new();
        while let Some(h) = s.peek() {
            order.push(s.slot(h).next_call_and_phase >> 2);
            s.erase(UpdateHandle(h));
            s.validate();
        }
        assert_eq!(order, vec![2, 3, 4, 5, 6, 7, 8, 10]);
    }

    /// The key is `(wake << 2) | phase` (`friend_getPriority`, UpdateModule.h): of two modules due on one
    /// frame, the `PHASE_INITIAL` one (an AI) is popped first whatever their registration order
    /// (GameLogic.cpp:3851-3905). Revert check: a key without the phase bits pops the earlier-registered module.
    #[test]
    fn the_phase_orders_the_modules_of_one_wake_frame() {
        let mut s = UpdateSchedule::default();
        let normal = s.register(ObjectId(1), Box::new(Dummy(5)), 1);
        let initial = s.register_ai(ObjectId(2), Box::new(Dummy(5)), 1);
        assert_eq!(s.next_call_frame(normal), 5, "the wake frame is the key's frame");
        assert_eq!(s.peek(), Some(initial.0), "PHASE_INITIAL pops before PHASE_NORMAL on the same frame");
    }

    /// `GameLogic::processDestroyList` erases one object's updates in heap-array order, last to first
    /// (GameLogic.cpp:2578-2609). For this layout the registration-order erase leaves a different heap, so the
    /// assertion below fails if the erase follows the registration order. Revert check: erase `erase_object`'s
    /// handles in registration order (sorted by handle) and the test fails.
    #[test]
    fn destroying_an_object_erases_its_updates_in_heap_order() {
        use crate::sleepy_families::FamilyKind;
        let keys = |s: &UpdateSchedule| s.heap.iter().map(|&h| s.slots[h as usize].next_call_and_phase).collect::<Vec<u32>>();
        // Six entries of three objects; the wake frames come from a fixed LCG, so every run tries the same layouts.
        let build = |wakes: &[u32]| {
            let mut s = UpdateSchedule::default();
            let owners = [1, 2, 1, 2, 1, 3];
            for (i, (&w, &o)) in wakes.iter().zip(owners.iter()).enumerate() {
                let kind = if i % 2 == 0 { FamilyKind::AutoHeal(0) } else { FamilyKind::Poisoned };
                s.register_family(ObjectId(o), kind, w, 1);
            }
            s
        };
        let mut seed = 7u32;
        let mut found = false;
        for _ in 0..500 {
            let wakes: Vec<u32> = (0..6)
                .map(|_| {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                    1 + (seed >> 8) % 100
                })
                .collect();
            let mut by_heap = build(&wakes);
            by_heap.erase_object(ObjectId(1));
            by_heap.validate();

            // The same erase, written out in the engine's order (the heap scan, last to first).
            let mut scan = build(&wakes);
            for h in scan.handles_of(ObjectId(1)).into_iter().rev() {
                scan.erase(h);
            }
            assert_eq!(keys(&by_heap), keys(&scan), "the object's updates are erased in heap-scan order");

            // The erase in registration order (what the port did before) leaves another heap for some layouts.
            let mut registered = build(&wakes);
            let mut mine: Vec<UpdateHandle> = registered.handles_of(ObjectId(1));
            mine.sort();
            for h in mine.into_iter().rev() {
                registered.erase(h);
            }
            if keys(&registered) != keys(&scan) {
                found = true;
                break;
            }
        }
        assert!(found, "a layout where the two erase orders differ");
    }

    #[test]
    fn awaken_moves_module_forward() {
        let mut s = UpdateSchedule::default();
        let a = s.register(ObjectId(1), Box::new(Dummy(100)), 1);
        let _b = s.register(ObjectId(2), Box::new(Dummy(50)), 1);
        s.awaken(a, 10, 11);
        s.validate();
        assert_eq!(s.peek(), Some(a.0));
    }
}
