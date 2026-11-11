//! `ThingTemplate`: the typed form of an `Object` / `ObjectReskin` block.
//!
//! Ported from `Core/GameEngine/Source/Common/Thing/ThingTemplate.cpp` (`s_objectFieldParseTable`,
//! `s_objectReskinFieldParseTable`, `parseModuleName`, `parseAddModule`/`RemoveModule`/`ReplaceModule`/
//! `InheritableModule`/`OverrideableByLikeKind`, `ModuleInfo::clearCopiedFromDefaultEntries`,
//! `copyFrom`, `setCopiedFromDefault`), `ThingFactory::newTemplate` / `parseObjectDefinition`,
//! `GeometryInfo` (`Common/System/Geometry.cpp`), `WeaponTemplateSet::parseWeaponTemplateSet`
//! (`GameLogic/Object/WeaponSet.cpp`) and `ArmorTemplateSet::parseArmorTemplateSet`.

use std::collections::BTreeMap;

use zr_ini::{Block, Field, Item};

use crate::diag::{Diag, WarningKind};
use crate::flags::{ArmorSetFlags, KindOfMask, Mask, NameEnum, WeaponSetFlags};
use crate::module::{interface_mask, parse_module_data, Module, ModuleData, ModuleKind};
use crate::module_table::iface;
use crate::names::*;
use crate::registry::{ArmorId, Ref, ThingId, WeaponId};
use crate::value::{parse_color, Args, LOGICFRAMES_PER_SECOND};
use crate::weapon::LEVEL_COUNT;

/// `GeometryInfo` plus the bounding radii `calcBoundingStuff` derives from it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    pub geometry_type: GeometryType,
    pub is_small: bool,
    pub height: f32,
    pub major_radius: f32,
    pub minor_radius: f32,
    pub bounding_circle_radius: f32,
    pub bounding_sphere_radius: f32,
}

impl Geometry {
    /// `GeometryInfo::set` (the constructor path: sphere/cylinder normalise the unused extents).
    pub fn new(t: GeometryType, is_small: bool, height: f32, major: f32, minor: f32) -> Self {
        let (major_radius, minor_radius, height) = match t {
            GeometryType::Sphere => (major, major, major),
            GeometryType::Cylinder => (major, major, height),
            GeometryType::Box => (major, minor, height),
        };
        let mut g = Geometry {
            geometry_type: t,
            is_small,
            height,
            major_radius,
            minor_radius,
            bounding_circle_radius: 0.0,
            bounding_sphere_radius: 0.0,
        };
        g.calc_bounding();
        g
    }

    /// `GeometryInfo::calcBoundingStuff`. The engine computes with doubles (`m_height*0.5`, `sqrt`) and
    /// narrows to `Real` on store; this does the same.
    pub fn calc_bounding(&mut self) {
        let (maj, min, h) = (self.major_radius as f64, self.minor_radius as f64, self.height as f64);
        match self.geometry_type {
            GeometryType::Sphere => {
                self.bounding_sphere_radius = self.major_radius;
                self.bounding_circle_radius = self.major_radius;
            }
            GeometryType::Cylinder => {
                self.bounding_circle_radius = self.major_radius;
                let mut s = h * 0.5;
                if s < maj {
                    s = maj;
                }
                self.bounding_sphere_radius = s as f32;
            }
            GeometryType::Box => {
                self.bounding_circle_radius = (maj * maj + min * min).sqrt() as f32;
                let hh = h * 0.5;
                self.bounding_sphere_radius = (maj * maj + min * min + hh * hh).sqrt() as f32;
            }
        }
    }
}

/// An armor set (`ArmorTemplateSet`): the armor used while the object has these conditions.
#[derive(Debug, Clone, PartialEq)]
pub struct ArmorSet {
    pub conditions: ArmorSetFlags,
    /// `None` for `Armor = None`.
    pub armor: Option<Ref<ArmorId>>,
    /// `DamageFX` name; `None` for `DamageFX = None`.
    pub damage_fx: Option<String>,
}

/// A weapon set (`WeaponTemplateSet`).
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponSet {
    pub conditions: WeaponSetFlags,
    /// Indexed by [`WeaponSlot`]; `None` for `None`.
    pub weapons: [Option<Ref<WeaponId>>; 3],
    /// `AutoChooseSources` per slot (default: all sources).
    pub auto_choose_sources: [Mask<CommandSource>; 3],
    /// `PreferredAgainst` per slot.
    pub preferred_against: [KindOfMask; 3],
    pub share_weapon_reload_time: bool,
    pub weapon_lock_shared_across_sets: bool,
}

impl WeaponSet {
    fn new() -> Self {
        WeaponSet {
            conditions: Mask::EMPTY,
            weapons: [None, None, None],
            // 0xffffffff: any command source may auto-choose
            auto_choose_sources: [Mask::from_bits(u32::MAX as u128); 3],
            preferred_against: [Mask::EMPTY; 3],
            share_weapon_reload_time: false,
            weapon_lock_shared_across_sets: false,
        }
    }
}

/// A `Prerequisites` entry: `Object = A B C` (any one of them) or `Science = X`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prerequisite {
    /// `Object = A B C`: the first is required unless one of the rest (OR-ed on) is owned.
    AnyOfObjects(Vec<String>),
    Science(String),
}

/// `MaxSimultaneousOfType`: a count (0 = unlimited) or `DeterminedBySuperweaponRestriction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaxSimultaneous {
    Count(u16),
    DeterminedBySuperweaponRestriction,
}

/// The typed `Object`. Defaults are the `ThingTemplate` constructor's; a new template then starts as
/// a copy of `DefaultThingTemplate` when that exists (`ThingFactory::newTemplate`).
#[derive(Debug, Clone, PartialEq)]
pub struct ThingTemplate {
    pub name: String,
    pub id: ThingId,
    /// `ObjectReskin NAME PARENT`: the parent this was copied from.
    pub reskinned_from: Option<String>,
    /// `DisplayName` label (a localisation key; not translated here).
    pub display_name: Option<String>,
    pub radar_priority: RadarPriority,
    pub transport_slot_count: u8,
    pub fence_width: f32,
    pub fence_x_offset: f32,
    pub is_bridge: bool,
    pub vision_range: f32,
    pub shroud_clearing_range: f32,
    pub shroud_reveal_to_all_range: f32,
    /// `PlacementViewAngle`, radians.
    pub placement_view_angle: f32,
    pub factory_exit_width: f32,
    pub factory_extra_bib_width: f32,
    /// Four entries: regular, veteran, elite, heroic. `-999` means "use the experience value".
    pub skill_point_value: [i32; LEVEL_COUNT],
    pub experience_value: [i32; LEVEL_COUNT],
    pub experience_required: [i32; LEVEL_COUNT],
    pub is_trainable: bool,
    pub enter_guard: bool,
    pub hijack_guard: bool,
    /// Default owning side name (`Side`).
    pub side: String,
    pub prerequisites: Vec<Prerequisite>,
    pub buildable: BuildableStatus,
    pub build_cost: u16,
    /// `BuildTime`, in seconds (`parseReal`; the engine multiplies by the frame rate when building).
    pub build_time: f32,
    pub refund_value: u16,
    pub build_completion: BuildCompletion,
    pub energy_production: i32,
    pub energy_bonus: i32,
    pub is_forbidden: bool,
    pub is_prerequisite: bool,
    /// `DisplayColor` as `[r, g, b, a]`.
    pub display_color: Option<[u8; 4]>,
    pub editor_sorting: EditorSorting,
    pub kind_of: KindOfMask,
    pub command_set: String,
    pub build_variations: Vec<String>,
    /// All `Behavior` / `Body` / `Draw` / `ClientUpdate` modules in definition order.
    pub modules: Vec<Module>,
    pub select_portrait: Option<String>,
    pub button_image: Option<String>,
    pub upgrade_cameos: [Option<String>; 5],
    /// `Voice*` / `Sound*` audio events by field name; `NoSound` removes the entry.
    pub sounds: BTreeMap<String, String>,
    /// `UnitSpecificSounds` (`Key = SoundName`); the whole map is replaced by each block.
    pub per_unit_sounds: BTreeMap<String, String>,
    /// `UnitSpecificFX` (`Key = FXListName`, `None` stored as `None`).
    pub per_unit_fx: BTreeMap<String, Option<String>>,
    pub scale: f32,
    pub geometry: Geometry,
    pub shadow_type: Mask<ShadowType>,
    pub shadow_size_x: f32,
    pub shadow_size_y: f32,
    pub shadow_offset_x: f32,
    pub shadow_offset_y: f32,
    pub shadow_texture: String,
    /// `OcclusionDelay`, frames (default 3 seconds).
    pub occlusion_delay: u32,
    pub instance_scale_fuzziness: f32,
    pub structure_rubble_height: u8,
    pub threat_value: u16,
    pub max_simultaneous_of_type: MaxSimultaneous,
    pub max_simultaneous_link_key: Option<String>,
    pub crusher_level: u8,
    pub crushable_level: u8,
    pub armor_sets: Vec<ArmorSet>,
    pub weapon_sets: Vec<WeaponSet>,
    pub(crate) armor_copied_from_default: bool,
    pub(crate) weapons_copied_from_default: bool,
}

/// `ModuleParseMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModuleMode {
    Normal,
    AddRemoveReplace,
    Inheritable,
    OverrideableByLikeKind,
}

/// State carried through one `Object` block parse.
struct Parse<'a> {
    d: &'a mut Diag,
    gravity: f32,
    mode: ModuleMode,
    /// While in `ReplaceModule`: `(class name, tag)` of the module being replaced.
    replacing: Option<(String, String)>,
    /// `ObjectReskin` uses the much smaller reskin field table.
    reskin: bool,
    /// `INI_LOAD_CREATE_OVERRIDES` (a map's `map.ini`).
    overrides: bool,
}

impl ThingTemplate {
    /// `ThingTemplate::ThingTemplate()`.
    pub fn new(name: &str, id: ThingId) -> Self {
        ThingTemplate {
            name: name.to_string(),
            id,
            reskinned_from: None,
            display_name: None,
            radar_priority: RadarPriority::Invalid,
            transport_slot_count: 0,
            fence_width: 0.0,
            fence_x_offset: 0.0,
            is_bridge: false,
            vision_range: 0.0,
            shroud_clearing_range: -1.0,
            shroud_reveal_to_all_range: -1.0,
            placement_view_angle: 0.0,
            factory_exit_width: 0.0,
            factory_extra_bib_width: 0.0,
            skill_point_value: [-999; LEVEL_COUNT],
            experience_value: [0; LEVEL_COUNT],
            experience_required: [0; LEVEL_COUNT],
            is_trainable: false,
            enter_guard: false,
            hijack_guard: false,
            side: String::new(),
            prerequisites: Vec::new(),
            buildable: BuildableStatus::Yes,
            build_cost: 0,
            build_time: 1.0,
            refund_value: 0,
            build_completion: BuildCompletion::AppearsAtRallyPoint,
            energy_production: 0,
            energy_bonus: 0,
            is_forbidden: false,
            is_prerequisite: false,
            display_color: None,
            editor_sorting: EditorSorting::None,
            kind_of: Mask::EMPTY,
            command_set: String::new(),
            build_variations: Vec::new(),
            modules: Vec::new(),
            select_portrait: None,
            button_image: None,
            upgrade_cameos: Default::default(),
            sounds: BTreeMap::new(),
            per_unit_sounds: BTreeMap::new(),
            per_unit_fx: BTreeMap::new(),
            scale: 1.0,
            geometry: Geometry::new(GeometryType::Sphere, false, 1.0, 1.0, 1.0),
            shadow_type: Mask::EMPTY,
            shadow_size_x: 0.0,
            shadow_size_y: 0.0,
            shadow_offset_x: 0.0,
            shadow_offset_y: 0.0,
            shadow_texture: String::new(),
            occlusion_delay: 3 * LOGICFRAMES_PER_SECOND,
            instance_scale_fuzziness: 0.0,
            structure_rubble_height: 0,
            threat_value: 0,
            max_simultaneous_of_type: MaxSimultaneous::Count(0),
            max_simultaneous_link_key: None,
            crusher_level: 0,
            crushable_level: 255,
            armor_sets: Vec::new(),
            weapon_sets: Vec::new(),
            armor_copied_from_default: false,
            weapons_copied_from_default: false,
        }
    }

    /// `ThingTemplate::setCopiedFromDefault`.
    pub(crate) fn set_copied_from_default(&mut self) {
        self.armor_copied_from_default = true;
        self.weapons_copied_from_default = true;
        for m in &mut self.modules {
            m.copied_from_default = true;
        }
    }

    /// `ThingTemplate::copyFrom`: everything of `that` except this template's name and id.
    pub(crate) fn copy_from(&mut self, that: &ThingTemplate) {
        let (name, id) = (std::mem::take(&mut self.name), self.id);
        *self = that.clone();
        self.name = name;
        self.id = id;
    }

    pub fn is_kind_of(&self, k: KindOf) -> bool {
        self.kind_of.contains(k)
    }

    /// `isAnyKindOf`.
    pub fn is_any_kind_of(&self, m: KindOfMask) -> bool {
        self.kind_of.intersects(m)
    }

    /// The module with the given tag.
    pub fn module_by_tag(&self, tag: &str) -> Option<&Module> {
        self.modules.iter().find(|m| m.tag == tag)
    }

    /// Modules of one list, in definition order.
    pub fn modules_of(&self, kind: ModuleKind) -> impl Iterator<Item = &Module> {
        self.modules.iter().filter(move |m| m.kind == kind)
    }

    /// The body module, if any (there is normally exactly one).
    ///
    /// With several `Body` modules (an `AddModule` in a `map.ini` that does not replace the base one)
    /// the last wins, as in `Object::Object` (`m_body = body` for every body module in order).
    pub fn body(&self) -> Option<&crate::module::BodyData> {
        self.modules.iter().rev().find_map(|m| match &m.data {
            ModuleData::Body(b) => Some(b),
            _ => None,
        })
    }

    /// The AI module (`friend_getAIModuleInfo`): the first behavior module deriving `AIUpdateModuleData`.
    pub fn ai(&self) -> Option<&crate::module::AiUpdateData> {
        self.modules.iter().find_map(|m| match &m.data {
            ModuleData::Ai(a) => Some(a),
            _ => None,
        })
    }

    pub(crate) fn ai_mut(&mut self) -> Option<&mut crate::module::AiUpdateData> {
        self.modules.iter_mut().find_map(|m| match &mut m.data {
            ModuleData::Ai(a) => Some(a),
            _ => None,
        })
    }

    /// The locomotor names of one set (empty if the set or the AI module is absent).
    pub fn locomotor_set(&self, set: LocomotorSetType) -> &[Ref<crate::registry::LocomotorId>] {
        self.ai().and_then(|a| a.locomotor_sets.get(&set)).map_or(&[], Vec::as_slice)
    }

    /// The first armor set whose conditions are all satisfied by `flags` (the engine's
    /// `findArmorTemplateSet` picks the best match; this is the exact-match lookup).
    pub fn armor_set_for(&self, flags: ArmorSetFlags) -> Option<&ArmorSet> {
        self.armor_sets.iter().find(|s| s.conditions == flags)
    }

    pub fn weapon_set_for(&self, flags: WeaponSetFlags) -> Option<&WeaponSet> {
        self.weapon_sets.iter().find(|s| s.conditions == flags)
    }

    /// Applies an `Object` block's fields on top of this template (`initFromINI` with
    /// `s_objectFieldParseTable`), or an `ObjectReskin` block's with the reskin table.
    ///
    /// `overrides` is an `INI_LOAD_CREATE_OVERRIDES` load: the caller has already made the template
    /// a "copied from default" copy of the one being overridden (`ThingFactory::newOverride`).
    pub(crate) fn apply_block(&mut self, b: &Block, reskin: bool, overrides: bool, gravity: f32, d: &mut Diag) {
        let mut p = Parse { d, gravity, mode: ModuleMode::Normal, replacing: None, reskin, overrides };
        self.apply_items(&b.items, &mut p);
    }

    fn apply_items(&mut self, items: &[Item], p: &mut Parse) {
        for item in items {
            match item {
                Item::Field(f) => self.apply_field(f, p),
                Item::Block(b) => self.apply_child_block(b, p),
            }
        }
    }

    /// Keys valid in `s_objectReskinFieldParseTable`.
    fn reskin_allows(key: &str) -> bool {
        matches!(
            key,
            "Draw"
                | "Geometry"
                | "GeometryMajorRadius"
                | "GeometryMinorRadius"
                | "GeometryHeight"
                | "GeometryIsSmall"
                | "FenceWidth"
                | "FenceXOffset"
                | "MaxSimultaneousOfType"
                | "MaxSimultaneousLinkKey"
        )
    }

    fn apply_child_block(&mut self, b: &Block, p: &mut Parse) {
        if p.reskin && !Self::reskin_allows(&b.kind) {
            p.d.warn(
                WarningKind::UnknownField,
                &b.kind,
                b.line,
                format!("`{}` is not in the ObjectReskin field table", b.kind),
            );
            return;
        }
        match b.kind.as_str() {
            "Behavior" | "Body" | "Draw" | "ClientUpdate" => self.parse_module(b, p),
            "ArmorSet" => self.parse_armor_set(b, p.d),
            "WeaponSet" => self.parse_weapon_set(b, p.d),
            "Prerequisites" => self.parse_prerequisites(b, p.d),
            "UnitSpecificSounds" => {
                // `parsePerUnitSounds`: clear, then `map.insert` (first entry per key wins).
                self.per_unit_sounds.clear();
                for item in &b.items {
                    if let Item::Field(f) = item {
                        let name = Args::new(&f.value).string();
                        self.per_unit_sounds.entry(f.key.clone()).or_insert(name);
                    }
                }
            }
            "UnitSpecificFX" => {
                self.per_unit_fx.clear();
                for item in &b.items {
                    if let Item::Field(f) = item {
                        let n = Args::new(&f.value).string();
                        let v = if n.eq_ignore_ascii_case("None") { None } else { Some(n) };
                        self.per_unit_fx.entry(f.key.clone()).or_insert(v);
                    }
                }
            }
            "AddModule" | "InheritableModule" | "OverrideableByLikeKind" | "ReplaceModule" => {
                self.parse_module_edit_block(b, p)
            }
            _ => p.d.warn(WarningKind::UnknownField, &b.kind, b.line, format!("unknown block `{}`", b.kind)),
        }
    }

    /// `parseAddModule` / `parseInheritableModule` / `OverrideableByLikeKind` / `parseReplaceModule`.
    fn parse_module_edit_block(&mut self, b: &Block, p: &mut Parse) {
        if p.mode != ModuleMode::Normal {
            p.d.warn(WarningKind::Structure, &b.kind, b.line, "module edit blocks cannot be nested");
            return;
        }
        let old_mode = p.mode;
        match b.kind.as_str() {
            "AddModule" => p.mode = ModuleMode::AddRemoveReplace,
            "InheritableModule" => p.mode = ModuleMode::Inheritable,
            "OverrideableByLikeKind" => p.mode = ModuleMode::OverrideableByLikeKind,
            _ => {
                // ReplaceModule TAG: drop the tagged module, then the nested one must be the same class
                // with a new tag.
                let Some(tag) = b.header.first() else {
                    p.d.warn(WarningKind::Structure, "ReplaceModule", b.line, "missing module tag");
                    return;
                };
                let Some(removed) = self.remove_module(tag) else {
                    p.d.warn(WarningKind::Structure, "ReplaceModule", b.line, format!("module `{tag}` not found"));
                    return;
                };
                p.mode = ModuleMode::AddRemoveReplace;
                p.replacing = Some((removed, tag.clone()));
            }
        }
        p.d.push_scope(b.kind.clone());
        self.apply_items(&b.items, p);
        p.d.pop_scope();
        p.mode = old_mode;
        p.replacing = None;
    }

    /// `ThingTemplate::removeModuleInfo`: removes the tagged module from all lists, returning its class.
    fn remove_module(&mut self, tag: &str) -> Option<String> {
        let i = self.modules.iter().position(|m| m.tag == tag)?;
        Some(self.modules.remove(i).name)
    }

    /// `ModuleInfo::clearCopiedFromDefaultEntries` over all three lists. Returns whether anything
    /// was removed.
    fn clear_copied_from_default_entries(&mut self, mask: u32, new_name: &str) -> bool {
        use KindOf as K;
        let immune: KindOfMask = [
            K::Aircraft,
            K::Shrubbery,
            K::OptimizedTree,
            K::Structure,
            K::DrawableOnly,
            K::MobNexus,
            K::IgnoredInGui,
            K::ClearedByBuild,
            K::DefensiveWall,
            K::BallisticMissile,
            K::SupplySource,
            K::Boat,
            K::Inert,
            K::Bridge,
            K::LandmarkBridge,
            K::BridgeTower,
        ]
        .into_iter()
        .collect();
        let candidate: KindOfMask = [K::Score, K::Vehicle, K::Infantry, K::PortableStructure].into_iter().collect();
        let disallowed = self.is_any_kind_of(immune);
        let is_candidate = self.is_any_kind_of(candidate);
        let trainable = self.is_trainable;

        let before = self.modules.len();
        self.modules.retain(|m| {
            if m.interface_mask & mask == 0 || !m.copied_from_default {
                return true;
            }
            if m.inheritable {
                // the default auto-heal is useless to untrainable templates
                !(m.tag == "ModuleTag_DefaultAutoHealBehavior" && !trainable)
            } else if m.overrideable_by_like_kind {
                // keep unless the author named the same class, or this kind never gets such modules
                !(m.name == new_name || disallowed || !is_candidate)
            } else {
                false
            }
        });
        self.modules.len() != before
    }

    /// `ThingTemplate::parseModuleName` for one `Behavior` / `Body` / `Draw` / `ClientUpdate` block.
    fn parse_module(&mut self, b: &Block, p: &mut Parse) {
        let (Some(class), Some(tag)) = (b.header.first(), b.header.get(1)) else {
            // "Module tags are required and must be unique for all modules within an object definition"
            p.d.warn(WarningKind::Structure, &b.kind, b.line, "module needs a class name and a tag");
            return;
        };
        let (kind, is_body_key) = match b.kind.as_str() {
            "Body" => (ModuleKind::Behavior, true),
            "Behavior" => (ModuleKind::Behavior, false),
            "Draw" => (ModuleKind::Draw, false),
            _ => (ModuleKind::ClientUpdate, false),
        };
        let Some(mask) = interface_mask(class) else {
            p.d.warn(WarningKind::Structure, &b.kind, b.line, format!("module `{class}` is not registered"));
            return;
        };
        // the module factory is keyed by (name, type): the class must belong to this list
        let in_list = match kind {
            ModuleKind::Draw => mask & iface::DRAW != 0,
            ModuleKind::ClientUpdate => mask & iface::CLIENT_UPDATE != 0,
            ModuleKind::Behavior => mask & (iface::DRAW | iface::CLIENT_UPDATE) == 0,
        };
        if !in_list {
            p.d.warn(WarningKind::Structure, &b.kind, b.line, format!("`{class}` is not a {} module", kind.name()));
            return;
        }
        if is_body_key && mask & iface::BODY == 0 {
            p.d.warn(WarningKind::Structure, &b.kind, b.line, "only Body modules are allowed after `Body =`");
            return;
        }
        if !is_body_key && mask & iface::BODY != 0 {
            p.d.warn(WarningKind::Structure, &b.kind, b.line, "Body modules must use `Body =`");
            return;
        }

        if p.overrides {
            // "You must use AddModule to add modules in override INI files." (the engine throws)
            if p.mode != ModuleMode::AddRemoveReplace {
                p.d.warn(
                    WarningKind::Structure,
                    &b.kind,
                    b.line,
                    "modules in an override INI must sit inside AddModule / ReplaceModule; ignored",
                );
                return;
            }
        } else {
            // not an override load: defaults sharing an interface with the new module go away
            self.clear_copied_from_default_entries(mask, class);
        }

        if p.mode == ModuleMode::AddRemoveReplace {
            if let Some((old_name, old_tag)) = &p.replacing {
                if old_name != class {
                    p.d.warn(
                        WarningKind::Structure,
                        &b.kind,
                        b.line,
                        format!("ReplaceModule must replace a module with another {old_name}, not {class}"),
                    );
                    return;
                }
                if old_tag == tag {
                    p.d.warn(WarningKind::Structure, &b.kind, b.line, "ReplaceModule needs a new, unique tag");
                    return;
                }
            }
        }

        let data = parse_module_data(class, tag, b, p.gravity, p.d);
        if matches!(data, ModuleData::Ai(_)) {
            // a new AI module replaces any earlier one (and the locomotor sets stored on it)
            self.modules.retain(|m| !(m.kind == kind && m.is_ai()));
        }
        self.modules.push(Module {
            kind,
            name: class.clone(),
            tag: tag.clone(),
            interface_mask: mask,
            inheritable: p.mode == ModuleMode::Inheritable,
            overrideable_by_like_kind: p.mode == ModuleMode::OverrideableByLikeKind,
            copied_from_default: false,
            data,
        });
    }

    fn parse_armor_set(&mut self, b: &Block, d: &mut Diag) {
        if self.armor_copied_from_default {
            self.armor_copied_from_default = false;
            self.armor_sets.clear();
        }
        let mut set = ArmorSet { conditions: Mask::EMPTY, armor: None, damage_fx: None };
        d.push_scope("ArmorSet");
        for item in &b.items {
            let Item::Field(f) = item else {
                d.warn(WarningKind::Structure, "", 0, "unexpected nested block");
                continue;
            };
            let mut a = Args::new(&f.value);
            let r: Result<bool, String> = (|| {
                match f.key.as_str() {
                    "Conditions" => set.conditions = set.conditions.parse(a.rest())?,
                    "Armor" => {
                        let n = a.next()?;
                        set.armor = if n.eq_ignore_ascii_case("None") { None } else { Some(Ref::named(n.to_string())) };
                    }
                    "DamageFX" => {
                        let n = a.next()?;
                        set.damage_fx = if n.eq_ignore_ascii_case("None") { None } else { Some(n.to_string()) };
                    }
                    _ => return Ok(false),
                }
                Ok(true)
            })();
            match r {
                Ok(true) => {}
                Ok(false) => d.unknown_field(f),
                Err(e) => d.bad_value(f, e),
            }
        }
        d.pop_scope();
        self.armor_sets.push(set);
    }

    fn parse_weapon_set(&mut self, b: &Block, d: &mut Diag) {
        if self.weapons_copied_from_default {
            self.weapons_copied_from_default = false;
            self.weapon_sets.clear();
        }
        let mut set = WeaponSet::new();
        d.push_scope("WeaponSet");
        for item in &b.items {
            let Item::Field(f) = item else {
                d.warn(WarningKind::Structure, "", 0, "unexpected nested block");
                continue;
            };
            let mut a = Args::new(&f.value);
            let r: Result<bool, String> = (|| {
                match f.key.as_str() {
                    "Conditions" => set.conditions = set.conditions.parse(a.rest())?,
                    "Weapon" => {
                        let slot: WeaponSlot = a.index()?;
                        let n = a.next()?;
                        set.weapons[slot.index()] =
                            if n.eq_ignore_ascii_case("None") { None } else { Some(Ref::named(n.to_string())) };
                    }
                    "AutoChooseSources" => {
                        let slot: WeaponSlot = a.index()?;
                        let cur = set.auto_choose_sources[slot.index()];
                        set.auto_choose_sources[slot.index()] = cur.parse(a.rest())?;
                    }
                    "PreferredAgainst" => {
                        let slot: WeaponSlot = a.index()?;
                        let cur = set.preferred_against[slot.index()];
                        set.preferred_against[slot.index()] = cur.parse(a.rest())?;
                    }
                    "ShareWeaponReloadTime" => set.share_weapon_reload_time = a.boolean()?,
                    "WeaponLockSharedAcrossSets" => set.weapon_lock_shared_across_sets = a.boolean()?,
                    _ => return Ok(false),
                }
                Ok(true)
            })();
            match r {
                Ok(true) => {}
                Ok(false) => d.unknown_field(f),
                Err(e) => d.bad_value(f, e),
            }
        }
        d.pop_scope();
        self.weapon_sets.push(set);
    }

    fn parse_prerequisites(&mut self, b: &Block, d: &mut Diag) {
        d.push_scope("Prerequisites");
        for item in &b.items {
            let Item::Field(f) = item else {
                d.warn(WarningKind::Structure, "", 0, "unexpected nested block");
                continue;
            };
            let mut a = Args::new(&f.value);
            match f.key.as_str() {
                "Object" => {
                    let names: Vec<String> = a.rest().iter().map(|s| s.to_string()).collect();
                    self.prerequisites.push(Prerequisite::AnyOfObjects(names));
                }
                "Science" => match a.next() {
                    Ok(s) => self.prerequisites.push(Prerequisite::Science(s.to_string())),
                    Err(e) => d.bad_value(f, e),
                },
                _ => d.unknown_field(f),
            }
        }
        d.pop_scope();
    }

    fn apply_field(&mut self, f: &Field, p: &mut Parse) {
        if p.reskin && !Self::reskin_allows(&f.key) {
            p.d.warn(
                WarningKind::UnknownField,
                &f.key,
                f.line,
                format!("`{}` is not in the ObjectReskin field table", f.key),
            );
            return;
        }
        if f.key == "RemoveModule" {
            // `parseRemoveModule`: only at top level; the named module must exist.
            let r = if p.mode != ModuleMode::Normal {
                Err("RemoveModule is not allowed inside a module edit block".to_string())
            } else {
                let mut a = Args::new(&f.value);
                a.next().and_then(|tag| {
                    self.remove_module(tag).map(|_| ()).ok_or_else(|| format!("RemoveModule: module `{tag}` not found"))
                })
            };
            if let Err(e) = r {
                p.d.warn(WarningKind::Structure, &f.key, f.line, e);
            }
            return;
        }
        if f.key == "Locomotor" {
            self.parse_locomotor_field(f, p.d);
            return;
        }
        let mut a = Args::new(&f.value);
        match self.scalar_field(&f.key, &f.value, &mut a) {
            Ok(true) => {}
            Ok(false) => p.d.unknown_field(f),
            Err(e) => p.d.bad_value(f, e),
        }
    }

    /// `AIUpdateModuleData::parseLocomotorSet`: attaches `SET_x name...` to the AI module.
    fn parse_locomotor_field(&mut self, f: &Field, d: &mut Diag) {
        let mut a = Args::new(&f.value);
        let set: LocomotorSetType = match a.index() {
            Ok(s) => s,
            Err(e) => return d.bad_value(f, e),
        };
        let names: Vec<Ref<crate::registry::LocomotorId>> = a
            .rest()
            .iter()
            .filter(|t| !t.is_empty() && !t.eq_ignore_ascii_case("None"))
            .map(|t| Ref::named(t.to_string()))
            .collect();
        let tname = self.name.clone();
        let Some(ai) = self.ai_mut() else {
            d.warn(
                WarningKind::Structure,
                &f.key,
                f.line,
                format!("locomotor specified for {tname} without an AIUpdate module"),
            );
            return;
        };
        if ai.locomotor_sets.get(&set).is_some_and(|v| !v.is_empty()) {
            d.warn(WarningKind::Structure, &f.key, f.line, "re-specifying a LocomotorSet is not allowed");
            return;
        }
        ai.locomotor_sets.insert(set, names);
    }

    /// The scalar rows of `s_objectFieldParseTable`.
    fn scalar_field(&mut self, key: &str, value: &str, a: &mut Args) -> Result<bool, String> {
        match key {
            "DisplayName" => self.display_name = Some(a.next()?.to_string()),
            "RadarPriority" => self.radar_priority = a.index()?,
            "TransportSlotCount" => self.transport_slot_count = a.u8()?,
            "FenceWidth" => self.fence_width = a.real()?,
            "FenceXOffset" => self.fence_x_offset = a.real()?,
            "IsBridge" => self.is_bridge = a.boolean()?,
            "VisionRange" => self.vision_range = a.real()?,
            "ShroudClearingRange" => self.shroud_clearing_range = a.real()?,
            "ShroudRevealToAllRange" => self.shroud_reveal_to_all_range = a.real()?,
            "PlacementViewAngle" => self.placement_view_angle = a.angle()?,
            "FactoryExitWidth" => self.factory_exit_width = a.real()?,
            "FactoryExtraBibWidth" => self.factory_extra_bib_width = a.real()?,
            "SkillPointValue" => self.skill_point_value = int_list(a)?,
            "ExperienceValue" => self.experience_value = int_list(a)?,
            "ExperienceRequired" => self.experience_required = int_list(a)?,
            "IsTrainable" => self.is_trainable = a.boolean()?,
            "EnterGuard" => self.enter_guard = a.boolean()?,
            "HijackGuard" => self.hijack_guard = a.boolean()?,
            "Side" => self.side = a.string(),
            "Buildable" => self.buildable = a.index()?,
            "BuildCost" => self.build_cost = a.u16()?,
            "BuildTime" => self.build_time = a.real()?,
            "RefundValue" => self.refund_value = a.u16()?,
            "BuildCompletion" => self.build_completion = a.index()?,
            "EnergyProduction" => self.energy_production = a.int()?,
            "EnergyBonus" => self.energy_bonus = a.int()?,
            "IsForbidden" => self.is_forbidden = a.boolean()?,
            "IsPrerequisite" => self.is_prerequisite = a.boolean()?,
            "DisplayColor" => self.display_color = Some(parse_color(value)?),
            "EditorSorting" => self.editor_sorting = a.index()?,
            "KindOf" => self.kind_of = self.kind_of.parse(a.rest())?,
            "CommandSet" => self.command_set = a.string(),
            "BuildVariations" => self.build_variations = a.rest().iter().map(|s| s.to_string()).collect(),
            "SelectPortrait" => self.select_portrait = Some(a.string()),
            "ButtonImage" => self.button_image = Some(a.string()),
            "UpgradeCameo1" | "UpgradeCameo2" | "UpgradeCameo3" | "UpgradeCameo4" | "UpgradeCameo5" => {
                let i = (key.as_bytes()[key.len() - 1] - b'1') as usize;
                self.upgrade_cameos[i] = Some(a.string());
            }
            "VoiceSelect" | "VoiceGroupSelect" | "VoiceMove" | "VoiceAttack" | "VoiceEnter" | "VoiceFear"
            | "VoiceSelectElite" | "VoiceCreated" | "VoiceTaskUnable" | "VoiceTaskComplete" | "VoiceMeetEnemy"
            | "VoiceGarrison" | "VoiceDefect" | "VoiceAttackSpecial" | "VoiceAttackAir" | "VoiceGuard"
            | "SoundMoveStart" | "SoundMoveStartDamaged" | "SoundMoveLoop" | "SoundMoveLoopDamaged"
            | "SoundAmbient" | "SoundAmbientDamaged" | "SoundAmbientReallyDamaged" | "SoundAmbientRubble"
            | "SoundStealthOn" | "SoundStealthOff" | "SoundCreated" | "SoundOnDamaged" | "SoundOnReallyDamaged"
            | "SoundEnter" | "SoundExit" | "SoundPromotedVeteran" | "SoundPromotedElite" | "SoundPromotedHero"
            | "SoundFallingFromPlane" => {
                // `parseDynamicAudioEventRTS`: `NoSound` clears the event.
                let t = a.next()?;
                if t.eq_ignore_ascii_case("NoSound") {
                    self.sounds.remove(key);
                } else {
                    self.sounds.insert(key.to_string(), t.to_string());
                }
            }
            "Scale" => self.scale = a.real()?,
            "Geometry" => {
                self.geometry.geometry_type = a.index()?;
                self.geometry.calc_bounding();
            }
            "GeometryMajorRadius" => {
                self.geometry.major_radius = a.real()?;
                self.geometry.calc_bounding();
            }
            "GeometryMinorRadius" => {
                self.geometry.minor_radius = a.real()?;
                self.geometry.calc_bounding();
            }
            "GeometryHeight" => {
                self.geometry.height = a.real()?;
                self.geometry.calc_bounding();
            }
            "GeometryIsSmall" => {
                self.geometry.is_small = a.boolean()?;
                self.geometry.calc_bounding();
            }
            "Shadow" => self.shadow_type = self.shadow_type.parse(a.rest())?,
            "ShadowSizeX" => self.shadow_size_x = a.real()?,
            "ShadowSizeY" => self.shadow_size_y = a.real()?,
            "ShadowOffsetX" => self.shadow_offset_x = a.real()?,
            "ShadowOffsetY" => self.shadow_offset_y = a.real()?,
            "ShadowTexture" => self.shadow_texture = a.string(),
            "OcclusionDelay" => self.occlusion_delay = a.duration_frames()?,
            "InstanceScaleFuzziness" => self.instance_scale_fuzziness = a.real()?,
            "StructureRubbleHeight" => self.structure_rubble_height = a.u8()?,
            "ThreatValue" => self.threat_value = a.u16()?,
            "MaxSimultaneousOfType" => {
                let t = a.next()?;
                if t.eq_ignore_ascii_case("DeterminedBySuperweaponRestriction") {
                    self.max_simultaneous_of_type = MaxSimultaneous::DeterminedBySuperweaponRestriction;
                } else {
                    let v = crate::value::scan_int(t)?;
                    let v = u16::try_from(v).map_err(|_| format!("{v} is out of range for an unsigned short"))?;
                    self.max_simultaneous_of_type = MaxSimultaneous::Count(v);
                }
            }
            "MaxSimultaneousLinkKey" => self.max_simultaneous_link_key = Some(a.string()),
            "CrusherLevel" => self.crusher_level = a.u8()?,
            "CrushableLevel" => self.crushable_level = a.u8()?,
            _ => return Ok(false),
        }
        Ok(true)
    }
}

/// `ThingTemplate::parseIntList` with `LEVEL_COUNT` = 4 entries.
fn int_list(a: &mut Args) -> Result<[i32; LEVEL_COUNT], String> {
    let mut out = [0i32; LEVEL_COUNT];
    for v in out.iter_mut() {
        *v = a.int()?;
    }
    Ok(out)
}
