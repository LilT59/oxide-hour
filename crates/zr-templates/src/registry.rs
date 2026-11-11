//! The template registry: [`Templates::from_ini`] and by-name / by-id lookups.

use std::collections::BTreeMap;

use zr_ini::IniDb;

use crate::ai_data::AiData;
use crate::armor::ArmorTemplate;
use crate::diag::{Diag, Warning, WarningKind};
use crate::die::DieKind;
use crate::locomotor::LocomotorTemplate;
use crate::module::ModuleData;
use crate::names::{WeaponBonusCondition, WeaponBonusField};
use crate::ocl::{Nugget, ObjectCreationList};
use crate::thing::ThingTemplate;
use crate::upgrade::UpgradeTemplate;
use crate::value::Args;
use crate::weapon::WeaponTemplate;

macro_rules! id_type {
    ($(#[$m:meta])* $n:ident) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $n(pub u32);
        impl $n {
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

id_type!(
    /// Index of a [`ThingTemplate`], in order of first definition.
    ThingId
);
id_type!(
    /// Index of a [`WeaponTemplate`], in order of first definition.
    WeaponId
);
id_type!(
    /// Index of a [`LocomotorTemplate`], in order of first definition.
    LocomotorId
);
id_type!(
    /// Index of an [`ArmorTemplate`], in order of first definition.
    ArmorId
);
id_type!(
    /// Index of an [`UpgradeTemplate`], in order of first definition.
    UpgradeId
);
id_type!(
    /// Index of an [`ObjectCreationList`], in order of first definition.
    OclId
);

/// A by-name reference that is resolved to an id once every table is built.
/// `id` is `None` when no template of that name exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref<I> {
    pub name: String,
    pub id: Option<I>,
}

impl<I> Ref<I> {
    pub fn named(name: String) -> Self {
        Ref { name, id: None }
    }
}

/// All templates parsed from one INI database.
///
/// Storage is `Vec`s indexed by id with `BTreeMap` name indexes, so iteration order is
/// deterministic (first-definition order for the `Vec`s, name order for lookups).
#[derive(Debug, Default)]
pub struct Templates {
    things: Vec<ThingTemplate>,
    thing_ids: BTreeMap<String, ThingId>,
    weapons: Vec<WeaponTemplate>,
    weapon_ids: BTreeMap<String, WeaponId>,
    locomotors: Vec<LocomotorTemplate>,
    locomotor_ids: BTreeMap<String, LocomotorId>,
    armors: Vec<ArmorTemplate>,
    armor_ids: BTreeMap<String, ArmorId>,
    upgrades: Vec<UpgradeTemplate>,
    upgrade_ids: BTreeMap<String, UpgradeId>,
    ocls: Vec<ObjectCreationList>,
    ocl_ids: BTreeMap<String, OclId>,
    /// `GameData` `Gravity` (per frame^2; default -1), used for physics fall-speed conversion.
    pub gravity: f32,
    /// `GameData` `ValuePerSupplyBox`: money per supply box (default 100).
    pub value_per_supply_box: u32,
    /// `GameData` `AllowedHeightVariationForBuilding`: build flatness tolerance (default 10).
    pub allowed_height_variation_for_building: f32,
    /// `GameData` `LowEnergyPenaltyModifier`.
    pub low_energy_penalty_modifier: f32,
    /// `GameData` `MinLowEnergyProductionSpeed`.
    pub min_low_energy_production_speed: f32,
    /// `GameData` `MaxLowEnergyProductionSpeed`.
    pub max_low_energy_production_speed: f32,
    /// `GameData` `GroundStiffness` (`PhysicsBehavior::handleBounce`; default 0.5).
    pub ground_stiffness: f32,
    /// `GameData` `StructureStiffness` (`PhysicsBehavior::onCollide` bounces off immobile things).
    pub structure_stiffness: f32,
    /// `GameData` `WeaponBonus = CONDITION FIELD percent`: the global weapon bonus set
    /// (`GlobalData::m_weaponBonusSet`), applied before a weapon's own `WeaponBonus`.
    pub weapon_bonus: BTreeMap<(WeaponBonusCondition, WeaponBonusField), f32>,
    /// `GameData` `HealthBonus_<Level>` (`GlobalData::m_healthBonus`), fractions (default 1.0).
    pub health_bonus: [f32; 4],
    /// `SpecialPower` templates.
    pub special_powers: Vec<crate::special_power::SpecialPowerTemplate>,
    special_power_ids: BTreeMap<String, usize>,
    /// `Science` templates.
    pub sciences: Vec<crate::special_power::ScienceInfo>,
    science_ids: BTreeMap<String, usize>,
    /// `Rank` entries, in file order (`RankInfoStore`).
    pub ranks: Vec<crate::special_power::RankInfo>,
    /// The parsed `AIData` block (skirmish AI tunables, side info and build lists), if present.
    pub ai_data: Option<AiData>,
}

impl Templates {
    /// Builds every template from `db`, in the engine's order: armors, weapons, locomotors, then
    /// objects in load order (an `ObjectReskin` must come after its parent, as in the engine).
    ///
    /// Duplicate-definition semantics follow the engine's parse procedures:
    /// `Armor`: the last definition replaces earlier ones (`clear()` on every parse);
    /// `Weapon`: the first wins, later ones are ignored (non-override load);
    /// `Locomotor` and `Object`: later definitions overlay fields onto the existing one.
    pub fn from_ini(db: &IniDb) -> (Templates, Vec<Warning>) {
        Self::from_ini_overlay(db, usize::MAX)
    }

    /// Like [`Templates::from_ini`], where blocks at load-order position `first_override` and later
    /// (see [`IniDb::checkpoint`]) came from an `INI_LOAD_CREATE_OVERRIDES` load (a map's `map.ini`).
    ///
    /// Override semantics (`ThingFactory::newOverride`, `WeaponStore::newOverride`): the override
    /// starts as a copy of the template so far with `setCopiedFromDefault()`, then the block is
    /// parsed on top. For objects, module blocks must sit inside `AddModule` / `ReplaceModule`
    /// (`ThingTemplate::parseModuleName` rejects them otherwise) and default modules are not
    /// cleared by a newly added one. A repeated `Weapon` overlays instead of being rejected.
    pub fn from_ini_overlay(db: &IniDb, first_override: usize) -> (Templates, Vec<Warning>) {
        let mut t = Templates {
            gravity: -1.0,
            value_per_supply_box: 100,
            allowed_height_variation_for_building: 10.0,
            low_energy_penalty_modifier: 0.5,
            min_low_energy_production_speed: 0.25,
            max_low_energy_production_speed: 0.8,
            ground_stiffness: 0.5,
            structure_stiffness: 0.5,
            weapon_bonus: BTreeMap::new(),
            health_bonus: [1.0; 4],
            ..Default::default()
        };
        let mut d = Diag::default();

        // `GameData` fields the simulation needs (GlobalData.cpp `s_globalDataFieldParseTable`).
        for b in db.iter().filter(|b| b.kind == "GameData") {
            for f in b.fields() {
                let mut a = Args::new(&f.value);
                let r: Result<(), String> = (|| {
                    match f.key.as_str() {
                        "Gravity" => t.gravity = a.acceleration()?,
                        "ValuePerSupplyBox" => t.value_per_supply_box = a.uint()?,
                        "AllowedHeightVariationForBuilding" => t.allowed_height_variation_for_building = a.real()?,
                        "LowEnergyPenaltyModifier" => t.low_energy_penalty_modifier = a.real()?,
                        "MinLowEnergyProductionSpeed" => t.min_low_energy_production_speed = a.real()?,
                        "MaxLowEnergyProductionSpeed" => t.max_low_energy_production_speed = a.real()?,
                        "GroundStiffness" => t.ground_stiffness = a.real()?,
                        "StructureStiffness" => t.structure_stiffness = a.real()?,
                        "WeaponBonus" => {
                            let cond: WeaponBonusCondition = a.index()?;
                            let field: WeaponBonusField = a.index()?;
                            t.weapon_bonus.insert((cond, field), a.percent()?);
                        }
                        "HealthBonus_Regular" => t.health_bonus[0] = a.percent()?,
                        "HealthBonus_Veteran" => t.health_bonus[1] = a.percent()?,
                        "HealthBonus_Elite" => t.health_bonus[2] = a.percent()?,
                        "HealthBonus_Heroic" => t.health_bonus[3] = a.percent()?,
                        _ => return Ok(()),
                    }
                    Ok(())
                })();
                if let Err(e) = r {
                    d.push_scope("GameData");
                    d.bad_value(f, e);
                    d.pop_scope();
                }
            }
        }

        // `AIData` (skirmish AI tunables, side info and per-faction build lists).
        if let Some(b) = db.iter().find(|b| b.kind == "AIData") {
            t.ai_data = Some(AiData::from_block(b, &mut d));
        }

        for b in db.of_kind("Armor") {
            let a = ArmorTemplate::from_block(b, &mut d);
            t.add_armor(a);
        }

        for b in db.of_kind("Weapon") {
            let name = b.name().unwrap_or("");
            let defs = db.definitions_indexed("Weapon", name);
            let mut w = WeaponTemplate::from_block(defs[0].1, &mut d);
            for &(i, dup) in defs.iter().skip(1) {
                if i >= first_override {
                    w.apply_block(dup, &mut d);
                } else {
                    d.push_scope(format!("Weapon {name}"));
                    d.warn(WarningKind::DuplicateIgnored, "", dup.line, "duplicate Weapon definition ignored (engine rejects it)");
                    d.pop_scope();
                }
            }
            t.add_weapon(w);
        }

        for b in db.of_kind("Locomotor") {
            let name = b.name().unwrap_or("");
            let mut loco = LocomotorTemplate::new(name);
            for def in db.definitions("Locomotor", name) {
                loco.apply_block(def, &mut d);
            }
            t.add_locomotor(loco);
        }

        // `UpgradeCenter::parseUpgradeDefinition`: a repeated definition overlays the existing
        // template; a new one starts as a copy of `DefaultUpgrade` when present.
        for b in db.of_kind("Upgrade") {
            let name = b.name().unwrap_or("");
            let id = match t.upgrade_ids.get(name) {
                Some(&id) => id,
                None => {
                    let u = match t.upgrade_ids.get("DefaultUpgrade") {
                        Some(&def) => {
                            let mut c = t.upgrades[def.index()].clone();
                            c.name = name.to_string();
                            c
                        }
                        None => UpgradeTemplate::new(name),
                    };
                    let id = UpgradeId(t.upgrades.len() as u32);
                    t.upgrade_ids.insert(name.to_string(), id);
                    t.upgrades.push(u);
                    id
                }
            };
            // every definition overlays (`parseUpgradeDefinition` runs `initFromINI` on the
            // existing template), so apply them all in load order
            for def in db.definitions("Upgrade", name) {
                t.upgrades[id.index()].apply_block(def, &mut d);
            }
        }

        // `SpecialPowerStore::parseSpecialPowerDefinition`: a repeated definition replaces fields.
        for b in db.of_kind("SpecialPower") {
            let name = b.name().unwrap_or("");
            let p = crate::special_power::SpecialPowerTemplate::parse(name, b, &mut d);
            match t.special_power_ids.get(name) {
                Some(&i) => t.special_powers[i] = p,
                None => {
                    t.special_power_ids.insert(name.to_string(), t.special_powers.len());
                    t.special_powers.push(p);
                }
            }
        }

        // `ScienceStore::friend_parseScienceDefinition`.
        for b in db.of_kind("Science") {
            let name = b.name().unwrap_or("");
            let s = crate::special_power::ScienceInfo::parse(name, b, &mut d);
            match t.science_ids.get(name) {
                Some(&i) => t.sciences[i] = s,
                None => {
                    t.science_ids.insert(name.to_string(), t.sciences.len());
                    t.sciences.push(s);
                }
            }
        }

        // `RankInfoStore` (`Rank.ini`).
        for b in db.of_kind("Rank") {
            t.ranks.push(crate::special_power::RankInfo::parse(b, &mut d));
        }

        // `ObjectCreationListStore::parseObjectCreationListDefinition`: `clear()` then parse, so a
        // repeated definition replaces the earlier one entirely.
        for b in db.of_kind("ObjectCreationList") {
            let ocl = ObjectCreationList::from_block(b, &mut d);
            t.add_ocl(ocl);
        }

        t.build_things(db, first_override, &mut d);
        t.resolve(&mut d);
        (t, d.warnings)
    }

    fn add_armor(&mut self, a: ArmorTemplate) {
        match self.armor_ids.get(&a.name) {
            Some(&id) => self.armors[id.index()] = a,
            None => {
                let id = ArmorId(self.armors.len() as u32);
                self.armor_ids.insert(a.name.clone(), id);
                self.armors.push(a);
            }
        }
    }

    fn add_weapon(&mut self, w: WeaponTemplate) {
        let id = WeaponId(self.weapons.len() as u32);
        self.weapon_ids.insert(w.name.clone(), id);
        self.weapons.push(w);
    }

    fn add_locomotor(&mut self, l: LocomotorTemplate) {
        let id = LocomotorId(self.locomotors.len() as u32);
        self.locomotor_ids.insert(l.name.clone(), id);
        self.locomotors.push(l);
    }

    /// `ObjectCreationListStore::parseObjectCreationListDefinition`: a repeated name replaces the
    /// earlier list (`clear()`), otherwise a new id is assigned.
    fn add_ocl(&mut self, ocl: ObjectCreationList) {
        match self.ocl_ids.get(&ocl.name) {
            Some(&id) => self.ocls[id.index()] = ocl,
            None => {
                let id = OclId(self.ocls.len() as u32);
                self.ocl_ids.insert(ocl.name.clone(), id);
                self.ocls.push(ocl);
            }
        }
    }

    /// `ThingFactory::parseObjectDefinition` over every `Object` / `ObjectReskin` block in load order.
    fn build_things(&mut self, db: &IniDb, first_override: usize, d: &mut Diag) {
        for (index, b) in db.iter().enumerate().filter(|(_, b)| b.kind == "Object" || b.kind == "ObjectReskin") {
            let is_override = index >= first_override;
            let Some(name) = b.name() else {
                d.push_scope(b.kind.clone());
                d.warn(WarningKind::Structure, "", b.line, "definition without a name");
                d.pop_scope();
                continue;
            };
            let is_reskin = b.kind == "ObjectReskin";
            d.push_scope(format!("{} {}", b.kind, name));

            // find or create (`ThingFactory::newTemplate` copies `DefaultThingTemplate` when present)
            let id = match self.thing_ids.get(name) {
                Some(&id) => {
                    if is_override {
                        // `newOverride`: copy of the final override, `setCopiedFromDefault()`
                        self.things[id.index()].set_copied_from_default();
                    }
                    id
                }
                None => {
                    let id = ThingId(self.things.len() as u32);
                    let mut tt = match self.thing_ids.get("DefaultThingTemplate") {
                        Some(&def) => {
                            let mut c = self.things[def.index()].clone();
                            c.name = name.to_string();
                            c.id = id;
                            c.set_copied_from_default();
                            c
                        }
                        None => ThingTemplate::new(name, id),
                    };
                    tt.id = id;
                    self.thing_ids.insert(name.to_string(), id);
                    self.things.push(tt);
                    id
                }
            };

            if is_reskin {
                let Some(parent_name) = b.header.get(1) else {
                    d.warn(WarningKind::Structure, "", b.line, "ObjectReskin without a parent name");
                    d.pop_scope();
                    continue;
                };
                let Some(&parent) = self.thing_ids.get(parent_name.as_str()) else {
                    d.warn(
                        WarningKind::UnknownReference,
                        "",
                        b.line,
                        format!("ObjectReskin must come after the original Object `{parent_name}`"),
                    );
                    d.pop_scope();
                    continue;
                };
                let parent_copy = self.things[parent.index()].clone();
                let tt = &mut self.things[id.index()];
                tt.copy_from(&parent_copy);
                tt.set_copied_from_default();
                tt.reskinned_from = Some(parent_name.clone());
            }

            let gravity = self.gravity;
            self.things[id.index()].apply_block(b, is_reskin, is_override, gravity, d);
            d.pop_scope();
        }
    }

    /// Fills in every [`Ref`] id and reports names that do not exist.
    fn resolve(&mut self, d: &mut Diag) {
        let weapon_ids = &self.weapon_ids;
        let armor_ids = &self.armor_ids;
        let loco_ids = &self.locomotor_ids;
        let thing_ids = &self.thing_ids;
        let ocl_ids = &self.ocl_ids;

        fn fix<I: Copy>(
            r: &mut Ref<I>,
            ids: &BTreeMap<String, I>,
            what: &str,
            d: &mut Diag,
            field: &str,
            warn_missing: bool,
        ) {
            r.id = ids.get(&r.name).copied();
            if r.id.is_none() && warn_missing {
                d.warn(WarningKind::UnknownReference, field, 0, format!("{what} `{}` not found", r.name));
            }
        }

        for w in &mut self.weapons {
            d.push_scope(format!("Weapon {}", w.name));
            if let Some(r) = &mut w.projectile_object {
                fix(r, thing_ids, "object", d, "ProjectileObject", true);
            }
            if let Some(r) = &mut w.historic_bonus_weapon {
                fix(r, weapon_ids, "weapon", d, "HistoricBonusWeapon", true);
            }
            d.pop_scope();
        }

        for t in &mut self.things {
            d.push_scope(format!("Object {}", t.name));
            for s in &mut t.armor_sets {
                if let Some(r) = &mut s.armor {
                    fix(r, armor_ids, "armor", d, "ArmorSet > Armor", true);
                }
            }
            for s in &mut t.weapon_sets {
                for r in s.weapons.iter_mut().flatten() {
                    fix(r, weapon_ids, "weapon", d, "WeaponSet > Weapon", true);
                }
            }
            for m in &mut t.modules {
                match &mut m.data {
                    ModuleData::Ai(ai) => {
                        for v in ai.locomotor_sets.values_mut() {
                            for r in v {
                                fix(r, loco_ids, "locomotor", d, "Locomotor", true);
                            }
                        }
                    }
                    ModuleData::Physics(p) => {
                        // defaults name engine weapons that may not exist in every data set
                        for r in [&mut p.vehicle_crashes_into_building_weapon, &mut p.vehicle_crashes_into_non_building_weapon]
                            .into_iter()
                            .flatten()
                        {
                            fix(r, weapon_ids, "weapon", d, "PhysicsBehavior", false);
                        }
                    }
                    ModuleData::Die(die) => match &mut die.kind {
                        DieKind::InstantDeath { ocl, weapons, .. } => {
                            for r in ocl.iter_mut() {
                                fix(r, ocl_ids, "object creation list", d, "OCL", false);
                            }
                            for r in weapons.iter_mut() {
                                fix(r, weapon_ids, "weapon", d, "Weapon", true);
                            }
                        }
                        DieKind::CreateObject { ocl: Some(r), .. } => {
                            fix(r, ocl_ids, "object creation list", d, "CreationList", false);
                        }
                        DieKind::SlowDeath(sd) => {
                            for v in sd.ocl.values_mut() {
                                for r in v.iter_mut() {
                                    fix(r, ocl_ids, "object creation list", d, "OCL", false);
                                }
                            }
                            for v in sd.weapons.values_mut() {
                                for r in v.iter_mut() {
                                    fix(r, weapon_ids, "weapon", d, "Weapon", true);
                                }
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
            d.pop_scope();
        }

        for o in &mut self.ocls {
            d.push_scope(format!("ObjectCreationList {}", o.name));
            for nugget in &mut o.nuggets {
                match nugget {
                    Nugget::FireWeapon(fw) => fix(&mut fw.weapon, weapon_ids, "weapon", d, "Weapon", true),
                    Nugget::DeliverPayload(dp) => {
                        if let Some(r) = &mut dp.data.visible_payload_weapon_template {
                            fix(r, weapon_ids, "weapon", d, "VisiblePayloadWeaponTemplate", true);
                        }
                    }
                    _ => {}
                }
            }
            d.pop_scope();
        }
    }

    // ----- lookups -----------------------------------------------------------------------

    /// Case-sensitive, like `NameKeyGenerator::nameToKey`.
    pub fn thing_id(&self, name: &str) -> Option<ThingId> {
        self.thing_ids.get(name).copied()
    }
    pub fn thing(&self, name: &str) -> Option<&ThingTemplate> {
        self.thing_id(name).map(|i| &self.things[i.index()])
    }
    pub fn thing_by_id(&self, id: ThingId) -> &ThingTemplate {
        &self.things[id.index()]
    }
    pub fn things(&self) -> &[ThingTemplate] {
        &self.things
    }

    pub fn weapon_id(&self, name: &str) -> Option<WeaponId> {
        self.weapon_ids.get(name).copied()
    }
    pub fn weapon(&self, name: &str) -> Option<&WeaponTemplate> {
        self.weapon_id(name).map(|i| &self.weapons[i.index()])
    }
    pub fn weapon_by_id(&self, id: WeaponId) -> &WeaponTemplate {
        &self.weapons[id.index()]
    }
    pub fn weapons(&self) -> &[WeaponTemplate] {
        &self.weapons
    }

    pub fn locomotor_id(&self, name: &str) -> Option<LocomotorId> {
        self.locomotor_ids.get(name).copied()
    }
    pub fn locomotor(&self, name: &str) -> Option<&LocomotorTemplate> {
        self.locomotor_id(name).map(|i| &self.locomotors[i.index()])
    }
    pub fn locomotor_by_id(&self, id: LocomotorId) -> &LocomotorTemplate {
        &self.locomotors[id.index()]
    }
    pub fn locomotors(&self) -> &[LocomotorTemplate] {
        &self.locomotors
    }

    pub fn armor_id(&self, name: &str) -> Option<ArmorId> {
        self.armor_ids.get(name).copied()
    }
    pub fn armor(&self, name: &str) -> Option<&ArmorTemplate> {
        self.armor_id(name).map(|i| &self.armors[i.index()])
    }
    pub fn armor_by_id(&self, id: ArmorId) -> &ArmorTemplate {
        &self.armors[id.index()]
    }
    pub fn armors(&self) -> &[ArmorTemplate] {
        &self.armors
    }

    pub fn upgrade_id(&self, name: &str) -> Option<UpgradeId> {
        self.upgrade_ids.get(name).copied()
    }
    pub fn upgrade(&self, name: &str) -> Option<&UpgradeTemplate> {
        self.upgrade_id(name).map(|i| &self.upgrades[i.index()])
    }
    pub fn upgrade_by_id(&self, id: UpgradeId) -> &UpgradeTemplate {
        &self.upgrades[id.index()]
    }
    pub fn upgrades(&self) -> &[UpgradeTemplate] {
        &self.upgrades
    }

    /// `SpecialPowerStore::findSpecialPowerTemplate`.
    pub fn special_power(&self, name: &str) -> Option<&crate::special_power::SpecialPowerTemplate> {
        self.special_power_ids.get(name).map(|&i| &self.special_powers[i])
    }
    /// `ScienceStore::findScienceInfo`.
    pub fn science(&self, name: &str) -> Option<&crate::special_power::ScienceInfo> {
        self.science_ids.get(name).map(|&i| &self.sciences[i])
    }

    pub fn ocl_id(&self, name: &str) -> Option<OclId> {
        self.ocl_ids.get(name).copied()
    }
    pub fn ocl(&self, name: &str) -> Option<&ObjectCreationList> {
        self.ocl_id(name).map(|i| &self.ocls[i.index()])
    }
    pub fn ocl_by_id(&self, id: OclId) -> &ObjectCreationList {
        &self.ocls[id.index()]
    }
    pub fn ocls(&self) -> &[ObjectCreationList] {
        &self.ocls
    }
}
