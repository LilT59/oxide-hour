//! `RiderChangeContain`: the GLA Combat Cycle's rider table.
//!
//! Ported from `GameLogic/Object/Contain/RiderChangeContain.cpp` (`onContaining`, `onRemoving`,
//! `isValidContainerFor`, `update`) and the `ActiveBody.cpp:394-421` `DAMAGE_KILLPILOT` case. The
//! bike is a `TransportContain` (see `contain.rs`); this module adds the rider table:
//!
//! * Entering swaps the bike over to the rider's `RiderN` entry: the `WEAPONSET_RIDERn` flag, the
//!   `STATUS_RIDERn` object status, the model condition, and the locomotor set. The rider's
//!   veterancy moves to the bike, and the rider starts again at regular. The previous rider is put
//!   out first, without scuttling the bike.
//! * Leaving (or being killed) reverts those changes. A rider that leaves without a replacement
//!   scuttles the bike: it becomes unselectable and, when stopped, immobile. After `ScuttleDelay`
//!   frames the bike topples and dies. A scuttled bike takes no rider.
//! * A bike that dies takes its rider with it (`destroyObject`), and a rider killed inside still
//!   scuttles the bike (`Object::onDestroy` -> `removeFromContain`).
//! * Slow-death modules gated on `STATUS_RIDERn` select by the bike's status mask.
//!
//! * Entering sets the bike's `CommandSet` override (the control bar reads it through
//!   [`Object::command_set_override`]). Leaving does not clear it, as in the engine.
//! * A stealthed bike is revealed when a rider boards (`markAsDetected`). A bike's stealth grant
//!   passes on to its rider (`StealthUpdate::receiveGrant`, see `behaviors.rs`).
//!
//! `UseRiderStealth` is modelled in `behaviors.rs` (`stealth_owner`): the bike's stealth follows the rider's
//! `CAN_STEALTH`, stealth level and delay. Not modelled: the `m_payloadCreated` check (the sim always evacuates the previous rider, which is the
//! state after a bike's first update). The TheSuperHackers trainable copy (`RiderChangeContain.cpp:252`,
//! `!RETAIL_COMPATIBLE_CRC`) is left out: it changes retail gameplay (a Worker rider would stop
//! ranking the bike up), and AGENTS.md says retail wins in that case.

use crate::body::DamageInfo;
use crate::condition::DOOR_1_CLOSING;
use crate::damage::{DamageType, DeathType, HUGE_DAMAGE_AMOUNT};
use crate::experience::VeterancyLevel;
use crate::logic::GameLogic;
use crate::object::{status, Object, ObjectId};
use crate::template::LocomotorSet;

/// `MAX_RIDERS` (`RiderChangeContain.h`).
pub const MAX_RIDERS: usize = 8;

/// One `RiderN` entry (`RiderInfo`), in sim units.
#[derive(Clone, Debug, PartialEq)]
pub struct RiderSlot {
    /// `m_templateName`. The engine compares with `ThingTemplate::isEquivalentTo`; the sim compares
    /// template names (reskins and build variations are not modelled).
    pub template_name: String,
    /// `m_modelConditionFlagType` as a `ModelConditionFlags` bit; `None` for `NONE`.
    pub model_condition: Option<u32>,
    /// `m_weaponSetFlag` as its `WEAPONSET_*` bit; 0 for `NONE`.
    pub weapon_set_flag: u32,
    /// `m_objectStatusType` as [`status`] bits; 0 for `NONE`.
    pub object_status: u64,
    /// `m_locomotorSetType`, applied with `AIUpdateInterface::chooseLocomotorSet`.
    pub locomotor_set: LocomotorSet,
    /// `m_commandSet` (`RiderChangeContain.cpp:88`): the `CommandSet` name the bike's control bar shows
    /// while this rider is aboard (`Object::setCommandSetStringOverride`). `NONE` is kept as the name,
    /// as the engine keeps it (no such set, so no buttons).
    pub command_set: String,
}

/// `RiderChangeContainModuleData` (sim units), held by the bike's [`ObjectTemplate`](crate::ObjectTemplate).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RiderChangeTemplate {
    /// The filled `Rider1..Rider8` entries, in slot order.
    pub riders: Vec<RiderSlot>,
    /// `m_scuttleFrames` (`ScuttleDelay`, already in frames).
    pub scuttle_frames: u32,
    /// `m_scuttleState` as a `ModelConditionFlags` bit (`ScuttleStatus`, default `TOPPLED`).
    pub scuttle_state: Option<u32>,
}

/// Per-bike runtime state of `RiderChangeContain`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RiderRt {
    /// `m_scuttledOnFrame`. 0 means "not scuttled", which is also the value for a bike scuttled on
    /// frame 0 (the engine's sentinel, kept as is).
    pub scuttled_on_frame: u32,
    /// Model-condition bits set by the rider table and the scuttle state (`setModelConditionState`).
    /// OR'd into the derived conditions each frame, as `upgrade_conditions` is.
    pub conditions: u128,
    /// `Object::m_commandSetStringOverride`, set on enter. Empty means no override. `onRemoving` never
    /// clears it (`RiderChangeContain.cpp:269-351`), so a bike keeps its last rider's command set
    /// after the rider leaves, until the next rider replaces it.
    pub command_set: String,
}

impl Object {
    /// `Object::getCommandSetString`'s override half: the `CommandSet` set by a rider change
    /// (`RiderChangeContain::onContaining`), or `None` when the template's own set applies. The sim has
    /// no command-set table, so the client looks up the name (`zr-viewer` `hud::selected_command_set`).
    pub fn command_set_override(&self) -> Option<&str> {
        (!self.rider.command_set.is_empty()).then_some(self.rider.command_set.as_str())
    }
}

impl GameLogic {
    /// Is `container` a `RiderChangeContain` bike (the template has a rider table)?
    pub(crate) fn is_rider_change(&self, container: ObjectId) -> bool {
        self.find_object(container).is_some_and(|c| c.template.rider_change.is_some())
    }

    /// The `RiderN` entry `unit` matches, if `container` is a rider bike.
    fn rider_slot_for(&self, container: ObjectId, unit: ObjectId) -> Option<RiderSlot> {
        let table = self.find_object(container)?.template.rider_change.as_ref()?;
        let name = &self.find_object(unit)?.template.name;
        table.riders.iter().find(|r| r.template_name.eq_ignore_ascii_case(name)).cloned()
    }

    /// `RiderChangeContain::isValidContainerFor` (`RiderChangeContain.cpp:156`), beyond the transport
    /// rules: a scuttled bike takes no rider, and the rider's template must be in the table.
    pub(crate) fn rider_accepts(&self, container: ObjectId, unit: ObjectId) -> bool {
        let scuttled = self.find_object(container).is_some_and(|c| c.rider.scuttled_on_frame != 0);
        !scuttled && self.rider_slot_for(container, unit).is_some()
    }

    fn rider_set_condition(&mut self, id: ObjectId, bit: u32) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.rider.conditions |= 1u128 << bit;
        }
    }

    fn rider_clear_condition(&mut self, id: ObjectId, bit: u32) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.rider.conditions &= !(1u128 << bit);
        }
    }

    /// `Object::getExperienceTracker()->setExperienceAndLevel(0, FALSE)`: back to regular, no points.
    fn rider_reset_experience(&mut self, id: ObjectId) {
        self.set_veterancy_level(id, VeterancyLevel::Regular);
        if let Some(o) = self.objects.get_mut(&id) {
            o.experience.current = 0;
        }
    }

    /// `RiderChangeContain::onContaining` (`RiderChangeContain.cpp:185`), run by `add_to_contain` once
    /// `rider` is in the bike's list. The previous rider is put out first (`aiEvacuateInstantly`,
    /// while `m_containing`, so the bike does not scuttle).
    pub(crate) fn rider_on_containing(&mut self, container: ObjectId, rider: ObjectId) {
        let others: Vec<ObjectId> = self.contained_units(container).iter().copied().filter(|&x| x != rider).collect();
        for other in others {
            self.remove_from_contain_ex(container, other, true);
        }
        let Some(slot) = self.rider_slot_for(container, rider) else { return };
        if let Some(bit) = slot.model_condition {
            self.rider_set_condition(container, bit);
        }
        if slot.weapon_set_flag != 0 {
            self.set_weapon_set_flag(container, slot.weapon_set_flag);
        }
        if let Some(o) = self.objects.get_mut(&container) {
            o.status_bits |= slot.object_status;
            // `obj->setCommandSetStringOverride` (`RiderChangeContain.cpp:227`). Set even when the name is
            // empty, as the engine does; an empty override then reads as "no override".
            o.rider.command_set = slot.command_set.clone();
        }
        // `ai->chooseLocomotorSet` (`RiderChangeContain.cpp:234`).
        self.ai_choose_locomotor_set(container, slot.locomotor_set);
        // A stealthed bike is revealed by the rider (`RiderChangeContain.cpp:237-244`): `markAsDetected()` with the
        // default delay (0), so it is visible until the stealth owner's `StealthDelay` frames have passed
        // (`StealthUpdate.cpp:880-889, 901-905`).
        let stealthed = self.find_object(container).is_some_and(|o| o.status_mask(self.frame) & status::STEALTHED != 0);
        if stealthed {
            self.mark_detected(container, 0);
        }
        // Experience moves from the rider to the bike (`RiderChangeContain.cpp:252-255`). The
        // TheSuperHackers trainable copy (`:252`) is left out: see the module doc.
        if let Some(level) = self.find_object(rider).map(|r| r.experience.level) {
            self.set_veterancy_level(container, level);
            self.rider_reset_experience(rider);
        }
    }

    /// `RiderChangeContain::onRemoving` (`RiderChangeContain.cpp:269`). Returns `true` when the bike is
    /// dead: the rider is destroyed and the caller stops. Otherwise the rider's entry is reverted, and
    /// a rider that leaves without a replacement (`!replacing`) scuttles the bike.
    pub(crate) fn rider_on_removing(&mut self, container: ObjectId, rider: ObjectId, replacing: bool) -> bool {
        let Some(bike) = self.find_object(container) else { return false };
        if bike.is_dying() {
            // `TheGameLogic->destroyObject(rider)` (`RiderChangeContain.cpp:275`).
            if let Some(o) = self.objects.get_mut(&rider) {
                o.contained_by = None;
            }
            self.destroy_object(rider);
            return true;
        }
        let bike_level = bike.experience.level;
        if let Some(slot) = self.rider_slot_for(container, rider) {
            if let Some(bit) = slot.model_condition {
                self.rider_clear_condition(container, bit);
            }
            self.rider_clear_condition(container, DOOR_1_CLOSING);
            if slot.weapon_set_flag != 0 {
                self.clear_weapon_set_flag(container, slot.weapon_set_flag);
            }
            if let Some(o) = self.objects.get_mut(&container) {
                o.status_bits &= !slot.object_status;
            }
            // Experience goes back to the rider, and the bike starts again at regular.
            self.set_veterancy_level(rider, bike_level);
            self.rider_reset_experience(container);
        }
        if !replacing {
            self.rider_scuttle(container);
        }
        false
    }

    /// The scuttle half of `onRemoving` (`RiderChangeContain.cpp:342-349`): the bike cannot be
    /// selected or taken over, is immobile when stopped, and topples after `ScuttleDelay`.
    fn rider_scuttle(&mut self, container: ObjectId) {
        let now = self.frame;
        let Some(state) = self.find_object(container).and_then(|c| c.template.rider_change.as_ref()).map(|t| t.scuttle_state) else {
            return;
        };
        let Some(o) = self.objects.get_mut(&container) else { return };
        o.rider.scuttled_on_frame = now;
        o.status_bits |= status::UNSELECTABLE;
        let stopped = !o.is_ai_moving();
        if stopped {
            o.status_bits |= status::IMMOBILE;
        }
        if let Some(bit) = state {
            self.rider_set_condition(container, bit);
        }
    }

    /// `ActiveBody::attemptDamage` for `DAMAGE_KILLPILOT` on a rider bike (`ActiveBody.cpp:394-421`):
    /// a moving bike blows up, a stopped one puts its rider out (which scuttles it) and kills the
    /// rider.
    pub(crate) fn rider_kill_pilot(&mut self, bike: ObjectId, source: ObjectId) {
        let moving = self.find_object(bike).is_some_and(|o| o.is_ai_moving());
        let kill = DamageInfo { source, amount: HUGE_DAMAGE_AMOUNT, kind: DamageType::Unresistable };
        if moving {
            self.damage_object_with_death(bike, kill, DeathType::Normal);
            return;
        }
        let riders = self.contained_units(bike).to_vec();
        for &rider in &riders {
            self.remove_from_contain(bike, rider);
        }
        for rider in riders {
            self.damage_object_with_death(rider, kill, DeathType::Normal);
        }
    }

    /// `RiderChangeContain::update` (`RiderChangeContain.cpp:363-376`): a scuttled bike topples once its
    /// `ScuttleDelay` has passed (`kill(DAMAGE_UNRESISTABLE, DEATH_TOPPLED)`). Run from the bike's own
    /// contain slot, then `TransportContain::update`.
    pub(crate) fn rider_update_one(&mut self, container: ObjectId) {
        let now = self.frame;
        let Some(o) = self.objects.get(&container) else { return };
        if o.destroyed || o.is_dying() || o.rider.scuttled_on_frame == 0 {
            return;
        }
        let Some(delay) = o.template.rider_change.as_ref().map(|t| t.scuttle_frames) else { return };
        if o.rider.scuttled_on_frame + delay <= now {
            let kill = DamageInfo { source: ObjectId::INVALID, amount: HUGE_DAMAGE_AMOUNT, kind: DamageType::Unresistable };
            self.damage_object_with_death(container, kill, DeathType::Toppled);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::condition::bit_for_name;
    use crate::contain::{ContainKind, ContainTemplate, GarrisonFirePoints};
    use crate::die::DieMux;
    use crate::math::Coord3D;
    use crate::template::{kind_of, LocomotorKind, LocomotorParams};
    use std::sync::Arc;
    use crate::weapon::{WEAPONSET_RIDER1, WEAPONSET_RIDER2};
    use crate::{ObjectTemplate, TerrainLogic};

    fn bike_contain() -> ContainTemplate {
        ContainTemplate {
            kind: ContainKind::RiderChange,
            capacity: 1,
            allow_inside_kind_of: Some(kind_of::INFANTRY),
            forbid_inside_kind_of: 0,
            allow_allies_inside: true,
            allow_enemies_inside: true,
            allow_neutral_inside: true,
            passengers_allowed_to_fire: false,
            damage_percentage_to_units: 0.0,
            number_of_exit_paths: 1,
            exit_delay: 0,
            door_open_time: 1,
            scatter_nearby_on_exit: true,
            orient_like_container_on_exit: false,
            reset_mood_check_on_exit: false,
            health_regen: 0.0,
            do_i_heal_objects: false,
            heal_frames: 0.0,
            initial_payload: Vec::new(),
            tunnel_frames_for_full_heal: 1.0,
            is_enclosing_container: false,
            fire_points: GarrisonFirePoints::default(),
            passengers_in_turret: false,
            weapon_bonus_passed_to_passengers: false,
            immune_to_clear_building_attacks: false,
        }
    }

    fn slot(name: &str, model: &str, weapon: u32, st: u64) -> RiderSlot {
        RiderSlot {
            template_name: name.into(),
            model_condition: bit_for_name(model),
            weapon_set_flag: weapon,
            object_status: st,
            locomotor_set: LocomotorSet::Normal,
            command_set: String::new(),
        }
    }

    /// A synthetic combat bike: a vehicle with a rider table of two riders and a 30-frame scuttle.
    fn bike() -> ObjectTemplate {
        let mut t = ObjectTemplate::new("Bike");
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
        t.geometry.major_radius = 8.0;
        t.max_health = 200.0;
        t.initial_health = 200.0;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
        t.contain = Some(Arc::new(bike_contain()));
        t.rider_change = Some(Arc::new(RiderChangeTemplate {
            riders: vec![
                slot("Rebel", "RIDER1", WEAPONSET_RIDER1, status::RIDER1),
                slot("Terrorist", "RIDER2", WEAPONSET_RIDER2, status::RIDER2),
            ],
            scuttle_frames: 30,
            scuttle_state: bit_for_name("TOPPLED"),
        }));
        t
    }

    fn infantry(name: &str) -> ObjectTemplate {
        let mut t = ObjectTemplate::new(name);
        t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
        t.geometry.major_radius = 4.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 60.0, 120.0, 180.0));
        t.transport_slot_count = 1;
        t
    }

    /// Bike, Rebel, Terrorist and Ranger (not in the table), one frame in so the scuttle sentinel
    /// (frame 0) is not in play.
    fn setup() -> (GameLogic, ObjectId, ObjectId, ObjectId, ObjectId) {
        let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
        let at = Coord3D::new(100.0, 100.0, 0.0);
        let b = g.create_object(&Arc::new(bike()), at, 0.0, 0);
        let r = g.create_object(&Arc::new(infantry("Rebel")), at, 0.0, 0);
        let t = g.create_object(&Arc::new(infantry("Terrorist")), at, 0.0, 0);
        let ranger = g.create_object(&Arc::new(infantry("Ranger")), at, 0.0, 0);
        g.update();
        (g, b, r, t, ranger)
    }

    fn bike_status(g: &GameLogic, b: ObjectId) -> (u32, u64, u32) {
        let o = g.find_object(b).unwrap();
        (o.weapon_set_flags, o.status_mask(g.frame), o.rider.scuttled_on_frame)
    }

    #[test]
    fn enter_sets_the_rider_table_and_exit_reverts_it() {
        let (mut g, b, r, _, _) = setup();
        g.force_enter_contain(b, r);
        g.update();
        let (weapons, st, scuttled) = bike_status(&g, b);
        assert_ne!(weapons & WEAPONSET_RIDER1, 0, "WEAPONSET_RIDER1 set");
        assert_ne!(st & status::RIDER1, 0, "STATUS_RIDER1 set");
        assert_eq!(scuttled, 0, "a rider does not scuttle the bike");
        let rider_bit = bit_for_name("RIDER1").unwrap();
        assert!(g.model_conditions(b).unwrap().test(rider_bit), "MODELCONDITION_RIDER1 set");

        // Exit: the flags clear, and the bike scuttles (no replacement).
        g.remove_from_contain(b, r);
        g.update();
        let (weapons, st, scuttled) = bike_status(&g, b);
        assert_eq!(weapons & WEAPONSET_RIDER1, 0, "WEAPONSET_RIDER1 cleared");
        assert_eq!(st & status::RIDER1, 0, "STATUS_RIDER1 cleared");
        assert_ne!(scuttled, 0, "a riderless bike is scuttled");
        assert!(!g.model_conditions(b).unwrap().test(rider_bit), "MODELCONDITION_RIDER1 cleared");
        assert_ne!(st & status::UNSELECTABLE, 0, "a scuttled bike is unselectable");
        assert_ne!(st & status::IMMOBILE, 0, "a stopped scuttled bike is immobile");
    }

    #[test]
    fn only_listed_templates_may_enter() {
        let (g, b, r, t, ranger) = setup();
        assert!(g.is_valid_container_for(b, r, true), "a listed rider");
        assert!(g.is_valid_container_for(b, t, true), "the other listed rider");
        assert!(!g.is_valid_container_for(b, ranger, true), "an unlisted infantry is rejected");
    }

    #[test]
    fn a_second_rider_replaces_the_first_without_scuttling() {
        let (mut g, b, r, t, _) = setup();
        g.force_enter_contain(b, r);
        g.update();
        // The bike's capacity is 1, but the rider table ignores capacity: the terrorist takes over.
        g.force_enter_contain(b, t);
        g.update();
        assert_eq!(g.container_of(r), None, "the rebel was put out");
        assert_eq!(g.container_of(t), Some(b));
        assert_eq!(g.contained_units(b), &[t]);
        let (weapons, st, scuttled) = bike_status(&g, b);
        assert_eq!(weapons & WEAPONSET_RIDER1, 0, "the rebel's weapon set is gone");
        assert_ne!(weapons & WEAPONSET_RIDER2, 0, "the terrorist's weapon set is on");
        assert_eq!(st & status::RIDER1, 0);
        assert_ne!(st & status::RIDER2, 0);
        assert_eq!(scuttled, 0, "a replacement does not scuttle the bike");
        assert_eq!(st & status::UNSELECTABLE, 0);
    }

    #[test]
    fn a_scuttled_bike_topples_after_its_delay_and_takes_no_rider() {
        let (mut g, b, r, t, _) = setup();
        g.force_enter_contain(b, r);
        g.update();
        g.remove_from_contain(b, r);
        let scuttled_at = bike_status(&g, b).2;
        assert!(!g.is_valid_container_for(b, t, true), "a scuttled bike takes no rider");
        let mut frames = 0;
        while g.find_object(b).is_some_and(|o| !o.is_dying()) && frames < 200 {
            g.update();
            frames += 1;
        }
        assert!(g.find_object(b).is_none_or(|o| o.is_dying()), "the bike toppled");
        assert!(g.frame - scuttled_at >= 30, "not before ScuttleDelay: {} frames", g.frame - scuttled_at);
    }

    #[test]
    fn a_rider_killed_inside_scuttles_the_bike_and_reverts_its_status() {
        let (mut g, b, r, _, _) = setup();
        g.force_enter_contain(b, r);
        g.update();
        g.destroy_object(r);
        g.update();
        let (weapons, st, scuttled) = bike_status(&g, b);
        assert_eq!(weapons & WEAPONSET_RIDER1, 0);
        assert_eq!(st & status::RIDER1, 0);
        assert_ne!(scuttled, 0, "a rider that dies inside leaves the bike scuttled");
    }

    #[test]
    fn a_dead_bike_takes_its_rider_with_it() {
        let (mut g, b, r, _, _) = setup();
        g.force_enter_contain(b, r);
        g.update();
        let kill = DamageInfo { source: ObjectId::INVALID, amount: 9999.0, kind: DamageType::Unresistable };
        g.damage_object_with_death(b, kill, DeathType::Normal);
        g.update();
        assert!(g.find_object(r).is_none(), "the rider was destroyed with the bike");
    }

    #[test]
    fn kill_pilot_on_a_stopped_bike_puts_the_rider_out_and_kills_it() {
        let (mut g, b, r, _, _) = setup();
        g.force_enter_contain(b, r);
        g.update();
        let snipe = DamageInfo { source: ObjectId::INVALID, amount: 0.0, kind: DamageType::KillPilot };
        g.damage_object_with_death(b, snipe, DeathType::Normal);
        assert_eq!(g.container_of(r), None, "the rider left the bike");
        g.update();
        assert!(g.find_object(r).is_none(), "the rider was killed");
        let (_, _, scuttled) = bike_status(&g, b);
        assert_ne!(scuttled, 0, "the bike scuttled");
        assert!(g.find_object(b).is_some_and(|o| !o.is_dying()), "the bike itself survives until it topples");
    }

    #[test]
    fn rider_status_drives_slow_death_and_fire_weapon_collide_tests() {
        let (mut g, b, r, _, _) = setup();
        let required_rider1 = DieMux { death_types: u32::MAX, veterancy_levels: u32::MAX, exempt_status: 0, required_status: status::RIDER1 };
        let frame = g.frame;
        let st = g.find_object(b).unwrap().status_mask(frame);
        assert!(!required_rider1.is_applicable(DeathType::Normal, 0, st), "no rider, no RIDER1 module");
        assert!(crate::collide::fire_weapon_status_allows(st, 0, status::RIDER1), "forbidding RIDER1 passes when riderless");
        assert!(!crate::collide::fire_weapon_status_allows(st, status::RIDER1, 0), "requiring RIDER1 fails when riderless");

        g.force_enter_contain(b, r);
        g.update();
        let frame = g.frame;
        let st = g.find_object(b).unwrap().status_mask(frame);
        assert!(required_rider1.is_applicable(DeathType::Normal, 0, st), "the rider selects the RIDER1 module");
        assert!(crate::collide::fire_weapon_status_allows(st, status::RIDER1, 0));
        assert!(!crate::collide::fire_weapon_status_allows(st, 0, status::RIDER1));
    }
}
