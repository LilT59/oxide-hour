//! Veterancy experience (`GameLogic/Object/ExperienceTracker.cpp`).
//!
//! Ported: level thresholds (`ExperienceRequired`), gain (`addExperiencePoints`), kill value
//! (`getExperienceValue`, no experience for killing an ally), explicit `setVeterancyLevel` and
//! `setMinVeterancyLevel`, the `m_experienceScalar` and the optional `m_experienceSink`. Skipped:
//! the sink forwarding chain and `gainExpForLevel` convenience wrappers.

use crate::crc::Crc;
use crate::object::ObjectId;

/// `VeterancyLevel` (`LEVEL_REGULAR`..`LEVEL_HEROIC`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum VeterancyLevel {
    #[default]
    Regular = 0,
    Veteran = 1,
    Elite = 2,
    Heroic = 3,
}

impl VeterancyLevel {
    pub const COUNT: usize = 4;
    pub fn index(self) -> usize {
        self as usize
    }
    pub fn from_index(i: usize) -> Self {
        match i {
            1 => VeterancyLevel::Veteran,
            2 => VeterancyLevel::Elite,
            3 => VeterancyLevel::Heroic,
            _ => VeterancyLevel::Regular,
        }
    }
}

/// `ExperienceTracker`.
#[derive(Clone, Debug, PartialEq)]
pub struct ExperienceTracker {
    pub level: VeterancyLevel,
    pub current: i32,
    /// `m_isTrainable` (`ThingTemplate::isTrainable`, overridable by modules).
    pub is_trainable: bool,
    /// `m_experienceScalar` (default 1.0).
    pub experience_scalar: f32,
    /// `m_experienceSink`: gains are forwarded here when set and alive.
    pub sink: Option<ObjectId>,
}

impl ExperienceTracker {
    pub fn new(is_trainable: bool) -> Self {
        ExperienceTracker { level: VeterancyLevel::Regular, current: 0, is_trainable, experience_scalar: 1.0, sink: None }
    }

    /// `getExperienceValue`: no points for killing an ally.
    pub fn experience_value(template_value: i32, allied: bool) -> i32 {
        if allied {
            0
        } else {
            template_value
        }
    }

    /// `addExperiencePoints`; returns the level change `(old, new)` when the level moved.
    pub fn add_experience(&mut self, gain: i32, required: &[i32; VeterancyLevel::COUNT], scale: bool) -> Option<(VeterancyLevel, VeterancyLevel)> {
        if !self.is_trainable {
            return None;
        }
        let old = self.level;
        let amount = if scale { (gain as f32 * self.experience_scalar) as i32 } else { gain };
        self.current += amount;
        let mut level_index = 0usize;
        while level_index + 1 < VeterancyLevel::COUNT && self.current >= required[level_index + 1] {
            level_index += 1;
        }
        self.level = VeterancyLevel::from_index(level_index);
        (old != self.level).then_some((old, self.level))
    }

    /// `setVeterancyLevel`: sets the level and its minimum experience.
    pub fn set_veterancy_level(&mut self, level: VeterancyLevel, required: &[i32; VeterancyLevel::COUNT]) -> Option<(VeterancyLevel, VeterancyLevel)> {
        if self.level == level {
            return None;
        }
        let old = self.level;
        self.level = level;
        self.current = required[level.index()];
        Some((old, level))
    }

    /// `setMinVeterancyLevel`: only ever raises the level.
    pub fn set_min_veterancy_level(&mut self, level: VeterancyLevel, required: &[i32; VeterancyLevel::COUNT]) -> Option<(VeterancyLevel, VeterancyLevel)> {
        if self.level < level {
            self.set_veterancy_level(level, required)
        } else {
            None
        }
    }

    pub(crate) fn crc(&self, c: &mut Crc) {
        c.add_u32(self.level as u32);
        c.add_u32(self.current as u32);
        c.add_bool(self.is_trainable);
        c.add_f32(self.experience_scalar);
        c.add_u32(self.sink.map_or(0xffff_ffff, |x| x.0));
    }
}
