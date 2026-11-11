//! Dozer / worker construction (`GameLogic/Object/Update/AIUpdate/DozerAIUpdate.cpp`,
//! `Common/System/BuildAssistant.cpp` `isLocationLegalToBuild` / `buildObjectNow`,
//! `GameLogic` `MSG_DOZER_CONSTRUCT`).
//!
//! `Command::Construct` validates the location (map extent, flatness, cliff/water via the
//! pathfinder, immobile-object overlap, prerequisites, money), withdraws the cost, creates the
//! building with `UNDER_CONSTRUCTION` at 1 hp, and orders the dozer to walk to the site. Once the
//! dozer is idle at the site it advances the construction percent and health each frame by
//! `100 / calcTimeToBuild` and `maxHealth / calcTimeToBuild`; at 100% the status clears.
//! `Command::CancelConstruct` refunds and removes the unfinished building.

use crate::logic::GameLogic;
use crate::math::{cos, sin, Coord3D};
use crate::object::{ObjectId, PlayerIndex};
use crate::pathfind::CellType;
use crate::template::ObjectTemplate;

/// The object-overlap option of `BuildAssistant::isLocationLegalToBuild`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectOverlap {
    /// No object test (options without `NO_OBJECT_OVERLAP` / `NO_ENEMY_OBJECT_OVERLAP`).
    Ignore,
    /// `NO_ENEMY_OBJECT_OVERLAP`: only immobile enemies (or any immobile object with no builder) and enemy objects.
    EnemiesOnly,
    /// `NO_OBJECT_OVERLAP`: the full `isLocationClearOfObjects`.
    All,
}

/// The options of `BuildAssistant::isLocationLegalToBuild` that the port's callers pass (`CLEAR_PATH` and
/// `SHROUD_REVEALED` are handled by the callers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LegalBuildOptions {
    /// `TERRAIN_RESTRICTIONS`.
    pub terrain: bool,
    /// `NO_OBJECT_OVERLAP` / `NO_ENEMY_OBJECT_OVERLAP` / neither.
    pub objects: ObjectOverlap,
}

impl GameLogic {
    /// `PartitionManager::tryPosition`'s object test (PartitionManager.cpp:3862-3880): a radius-5 sphere at
    /// `pos` (`GeometryInfo(GEOMETRY_SPHERE, TRUE, 5, 5, 5)`) must not collide with any object's geometry
    /// (`PartitionFilterWouldCollide`, :5267), among the objects `iteratePotentialCollisions` returns (:3723:
    /// the bounding circle query, grown by 10%). `ignore` lists the objects C++ skips (`ignoreObject`,
    /// `sourceToPathToDest`).
    /// `PartitionManager::iteratePotentialCollisions` (`PartitionManager.cpp:3723-3743`): a tiny
    /// `GEOMETRY_SPHERE` (`major` and `minor` 5) at `pos`, turned by `angle` (the ring sample's angle:
    /// `tryPosition` builds `GeometryInfo geometry(GEOMETRY_SPHERE, TRUE, 5, 5, 5)` and passes the ring
    /// angle to `iteratePotentialCollisions`, so the sphere's squared 2D test rotates with it, `:479-486`).
    /// The query radius is the sphere's bounding sphere times 1.1 (`:3727-3729`).
    pub fn small_sphere_clear_at(&self, pos: &Coord3D, angle: f32, ignore: &[ObjectId]) -> bool {
        let sphere = Foot { pos: *pos, angle, boxed: false, sphere: true, major: 5.0, minor: 5.0, height: 5.0 };
        self.partition
            .objects_in_range(pos, 5.5, crate::partition::DistanceMode::FromBoundingSphere2D)
            .into_iter()
            .filter(|id| !ignore.contains(id))
            .filter_map(|id| self.objects.get(&id))
            .all(|o| !foot_collides(&sphere, &Foot::of(&o.position, o.angle, &o.template.geometry)))
    }

    /// The factory-side half of `BuildAssistant::canMakeUnit` (`BuildAssistant.cpp:1313-1343`) that
    /// `ProductionUpdate::queueCreateUnit` runs before it takes money (`ProductionUpdate.cpp:358`):
    /// a script-disabled or script-unpowered builder refuses (`:1315`), and so does one whose command set holds
    /// no `UNIT_BUILD` / `DOZER_CONSTRUCT` button for `unit` (`isPossibleToMakeUnit`, `:1249`). The script
    /// status bits mirror the `DISABLED_SCRIPT_DISABLED` / `DISABLED_SCRIPT_UNDERPOWERED` disable types
    /// (`Object::setScriptStatus`, `Object.cpp:1079-1106`). The command set comes from
    /// `TemplateSource::can_make_unit`; a source with no command-set data (hand-built test logic)
    /// does not refuse on it. The unit cap (`Player::canBuildMoreOfType`, `Player.cpp:2917`) is not ported.
    pub(crate) fn can_make_unit_from_factory(&self, factory: ObjectId, unit: &ObjectTemplate) -> bool {
        use crate::disabled::DisabledType;
        let Some(o) = self.objects.get(&factory) else { return false };
        if o.is_disabled_by_type(DisabledType::ScriptDisabled) || o.is_disabled_by_type(DisabledType::ScriptUnderpowered) {
            return false;
        }
        self.template_source.as_ref().and_then(|s| s.can_make_unit(&o.template.name, &unit.name)).unwrap_or(true)
    }

    /// `MSG_DOZER_CONSTRUCT`: start building `template_name` at `pos` with a dozer/worker.
    pub fn construct(&mut self, dozer: ObjectId, template_name: &str, pos: Coord3D, angle: f32) -> Option<ObjectId> {
        let t = self.template_source.as_ref().and_then(|s| s.find_template(template_name))?;
        let (player, busy, can_move) = match self.objects.get(&dozer) {
            Some(o) if !o.destroyed && o.template.locomotor.is_some() => (o.player, o.build_target.is_some(), true),
            _ => return None,
        };
        if busy || !can_move {
            return None;
        }
        // `DozerAIUpdate::construct` (`DozerAIUpdate.cpp:1671-1697`): an AI owner skips `canMakeUnit` and
        // `isLocationLegalToBuild` ("Just build it. The ai will validate, or cheat."). Only a human's order is checked.
        let ai_owner = self.is_ai_player(player);
        // `BuildAssistant::canMakeUnit`: prerequisites (command sets are not ported).
        if !ai_owner && !self.prerequisites_satisfied(player, &t) {
            return None;
        }
        // `buildObjectNow` (`BuildAssistant.cpp:346`) clears the removable objects under the footprint, then moves the
        // neutral and allied units out of it, before the dozer's own affordability and legality checks. It runs for
        // this order too (Object.cpp:5795).
        self.clear_removable_for_construction(&t, &pos, angle);
        // `BuildAssistant.cpp:348-353`: a human's order fails when a unit in the footprint could not be moved. An AI
        // builds regardless ("ai gets to cheat").
        let moved = self.move_objects_for_construction(&t, &pos, angle, player);
        if !moved && !self.is_ai_player(player) {
            return None;
        }
        let cost = self.calc_cost_to_build(player, &t);
        if self.player(player).map_or(0, |p| p.money) < cost {
            return None;
        }
        // A human's check (`DozerAIUpdate.cpp:1689`) passes the dozer as `builderObject` and a null `thePlayer`, so the
        // skirmish margin does not apply to it. The engine's CLEAR_PATH and SHROUD_REVEALED are not checked here.
        if !ai_owner && !self.is_location_legal_to_build_for(&t, &pos, angle, Some(player), None) {
            return None;
        }
        // `BuildAssistant::buildObjectNow`'s dozer branch (BuildAssistant.cpp, after `moveObjectsForConstruction`):
        // `ai->aiIdle(CMD_FROM_AI)` stops the dozer's current behaviour, entering `AI_IDLE` (one `AIIdleState::onEnter`
        // draw, AIStates.cpp:1327), then `ai->construct`. Only the AI's own build path (`buildStructureWithDozer`) goes
        // through `buildObjectNow`; a human's order is `DozerAIUpdate::construct` directly.
        if ai_owner {
            self.ai_idle(dozer, crate::ai_states::CommandSource::Ai);
        }
        if let Some(p) = self.players.get_mut(&player) {
            p.withdraw(cost);
        }

        // `buildObjectNow` / `DozerAIUpdate::construct`: create under construction at 1 hp.
        // `DozerAIUpdate::construct` (`DozerAIUpdate.cpp:1730`) / `WorkerAIUpdate::build`
        // (`WorkerAIUpdate.cpp:420`): `setPosition(pos)`, `setOrientation(angle)`, then
        // `flattenTerrain(obj)` and `adjustedPos.z = getGroundHeight(pos->x, pos->y)`.
        self.terrain.flatten_terrain(&pos, angle, &t.geometry);
        let mut site = pos;
        site.z = self.terrain.ground_height(pos.x, pos.y);
        let id = self.create_object_under_construction(&t, site, angle, player);
        if let Some(o) = self.objects.get_mut(&id) {
            o.construction_percent = 0.0;
            // `Object::setBuilder`: the dozer that started construction owns it.
            o.builder = Some(dozer);
        }
        if let Some(body) = self.objects.get_mut(&id).and_then(|o| o.body.as_mut()) {
            let max = body.max_health();
            let _ = body.attempt_damage(&crate::body::DamageInfo {
                source: ObjectId::INVALID,
                amount: max - 1.0,
                kind: crate::body::DamageType::Unresistable,
            });
        }

        // The dozer walks to its start point, then builds (`DozerAIUpdate::newTask`, `DozerAIUpdate.cpp:2018`).
        if let Some(o) = self.objects.get_mut(&dozer) {
            o.build_target = Some(id);
        }
        self.dozer_begin_build(dozer, id);
        Some(id)
    }

    /// Cancel a dozer's current construction: refund the cost and destroy the unfinished building.
    pub fn cancel_construct(&mut self, dozer: ObjectId) -> bool {
        let Some(target) = self.objects.get(&dozer).and_then(|o| o.build_target) else { return false };
        // `GameLogic::onDozerCancelConstruct` refunds `calcCostToBuild(msgPlayer)` (GameLogicDispatch.cpp:1944-1948).
        let (player, refund) = match self.objects.get(&target) {
            Some(o) if o.under_construction => (o.player, self.calc_cost_to_build(o.player, &o.template)),
            _ => (self.objects.get(&dozer).map_or(0, |o| o.player), 0),
        };
        if let Some(o) = self.objects.get_mut(&dozer) {
            o.build_target = None;
        }
        if refund > 0 {
            if let Some(p) = self.players.get_mut(&player) {
                p.deposit(refund);
            }
        }
        if self.objects.get(&target).is_some_and(|o| o.under_construction) {
            self.destroy_object(target);
        }
        true
    }

    /// Advance one dozer's build task (`DozerAIUpdate`'s `DOZER_DO_BUILD_AT_DOCK`), called from the
    /// dozer module's update right after its AI update (`DozerAIUpdate::update`, `DozerAIUpdate.cpp:1666`
    /// -> `DozerActionDoActionState::update`, `:520-534`). The action machine (`dozer.rs::
    /// dozer_build_step`) decides whether the dozer is at its dock and construction advances.
    pub(crate) fn dozer_build_action_update(&mut self, dozer: ObjectId, target: ObjectId) {
        // Wait until the dozer's build action reaches its dock (`DozerActionStateMachine`,
        // `dozer.rs`). A missing or finished structure falls through to the clearing below.
        if self.objects.get(&target).is_some_and(|b| b.under_construction) && !self.dozer_build_step(dozer, target) {
            return;
        }
        let Some(building) = self.objects.get(&target) else {
            if let Some(o) = self.objects.get_mut(&dozer) {
                o.build_target = None;
            }
            return;
        };
        if !building.under_construction {
            if let Some(o) = self.objects.get_mut(&dozer) {
                o.build_target = None;
            }
            return;
        }
        let player = building.player;
        let template = building.template.clone();
        let frames = self.effective_build_time_frames(&template, player).max(1);
        let percent_step = 100.0f32 / frames as f32;
        let health_step = template.max_health / frames as f32;
        let mut done = false;
        if let Some(o) = self.objects.get_mut(&target) {
            o.construction_percent += percent_step;
            if o.construction_percent >= 100.0 {
                o.construction_percent = 100.0;
                o.under_construction = false;
                done = true;
            }
            if let Some(body) = o.body.as_mut() {
                body.attempt_healing(health_step);
            }
        }
        if done {
            if let Some(o) = self.objects.get_mut(&dozer) {
                o.build_target = None;
            }
            // `Player::onStructureConstructionComplete` (`Player.cpp:1640-1641`): "Update the pathfind
            // footprint. Sometimes when rubble has to be removed to build, the initial footprint while
            // building is goofy." — `removeObjectFromPathfindMap` then `addObjectToPathfindMap`, i.e.
            // `classifyObjectFootprint(structure, false)` then `(true)`. The re-classification marks
            // every touched cell `UNINITIALIZED_ZONE` (`AIPathfind.cpp:4500-4518`); `updateZonesForModify`
            // gives each one a same-type neighbour's zone, and an obstacle footprint's inner cells stay
            // zone 0. It runs before the dozer's end-dock adjust (`DozerAIUpdate.cpp:664-674`), which
            // reads those zones. The re-add also re-runs the pinched/expand pass over the footprint.
            if let Some(o) = self.objects.get(&target) {
                let (pos, angle, tmpl) = (o.position, o.angle, o.template.clone());
                let terrain = &self.terrain;
                let pf = &mut self.pathfinder;
                pf.classify_object_footprint(terrain, target, &tmpl, &pos, angle, false);
                pf.classify_object_footprint(terrain, target, &tmpl, &pos, angle, true);
            }
            self.dozer_build_finished(dozer, target);
            // `DozerAIUpdate` (DozerAIUpdate.cpp:586) -> `Player::onStructureConstructionComplete`: score
            // the building and its build cost.
            self.score_structure_completed(target);
            // `SpecialPowerCreate::onBuildComplete` (SpecialPowerCreate.cpp:57) and
            // `GrantUpgradeCreate::onBuildComplete` (GrantUpgradeCreate.cpp:117) for the finished
            // building, then the upgrade modules, which were skipped while under construction
            // (Object.cpp:2481).
            self.create_modules_on_build_complete(target);
            self.update_upgrade_modules(target);
            // `AIPlayer::onStructureProduced`.
            if self.is_ai_player(player) {
                self.ai_on_structure_produced(target);
            }
        }
    }

    /// `BuildAssistant::iterateFootprint` (`BuildAssistant.cpp:542`): the footprint samples of `t` at `pos`
    /// and `angle`, `res` apart, in the engine's loop order (the edge is snapped onto the extent). A box
    /// samples its rectangle; a sphere or cylinder keeps the samples inside its circle. Each sample is at
    /// its world position with that position's ground height. Deviation: the engine calls
    /// `getGroundHeight` on the untransformed local `(x, y)`, which the port does not reproduce, because
    /// that reads the terrain at the map origin and rejects the shipped power plant site (height spread
    /// 13.9 against 10, while the engine places it).
    fn footprint_samples(&self, t: &ObjectTemplate, pos: &Coord3D, angle: f32, res: f32) -> Vec<Coord3D> {
        let boxed = matches!(t.geometry.shape, crate::template::GeometryShape::Box);
        let (half_h, half_w) = if boxed {
            (t.geometry.minor_radius, t.geometry.major_radius)
        } else {
            let r = t.geometry.bounding_circle_radius();
            (r, r)
        };
        let (c, s) = (cos(angle), sin(angle));
        let mut out = Vec::new();
        let mut y = -half_h;
        while y < half_h + res {
            if y > half_h {
                y = half_h;
            }
            let mut x = -half_w;
            while x < half_w + res {
                if x > half_w {
                    x = half_w;
                }
                if boxed || (x * x + y * y).sqrt() <= half_w {
                    let (wx, wy) = (pos.x + x * c - y * s, pos.y + x * s + y * c);
                    out.push(Coord3D::new(wx, wy, self.terrain.ground_height(wx, wy)));
                }
                x += res;
            }
            y += res;
        }
        out
    }

    /// `BuildAssistant::isLocationLegalToBuild` (terrain + object overlap; shroud is ignored).
    pub fn is_location_legal_to_build(&self, t: &ObjectTemplate, pos: &Coord3D, angle: f32) -> bool {
        self.is_location_legal_to_build_by(t, pos, angle, None)
    }

    /// `isLocationLegalToBuild` for `builder`, taken as both the `builderObject` and `thePlayer` arguments.
    /// Callers that pass a null `builderObject` (the AI's placement tests) should use
    /// [`Self::is_location_legal_to_build_for`] with `None` for the builder.
    pub fn is_location_legal_to_build_by(&self, t: &ObjectTemplate, pos: &Coord3D, angle: f32, builder: Option<PlayerIndex>) -> bool {
        self.is_location_legal_to_build_for(t, pos, angle, builder, builder)
    }

    /// `isLocationLegalToBuild(pos, build, angle, options, builderObject, thePlayer)` (`BuildAssistant.cpp:914`).
    /// `builder_object` gives the relationship test (a `None` is `NEUTRAL`); `the_player` gives the skirmish-AI
    /// margin (`thePlayer->isSkirmishAIPlayer()`). The two are separate in the engine: `DozerAIUpdate::construct`
    /// passes `(dozer, null)`, and `AIPlayer` passes `(null, m_player)`.
    pub fn is_location_legal_to_build_for(
        &self,
        t: &ObjectTemplate,
        pos: &Coord3D,
        angle: f32,
        builder_object: Option<PlayerIndex>,
        the_player: Option<PlayerIndex>,
    ) -> bool {
        self.is_location_legal_with(t, pos, angle, LegalBuildOptions { terrain: true, objects: ObjectOverlap::All }, builder_object, the_player)
    }

    /// `isLocationLegalToBuild` with the option subset the callers use: `terrain` is `TERRAIN_RESTRICTIONS`
    /// and `objects` is `NO_OBJECT_OVERLAP` (`All`), `NO_ENEMY_OBJECT_OVERLAP` (`EnemiesOnly`) or neither
    /// (`Ignore`). `CLEAR_PATH` is not part of this (the dozer callers test it themselves). The engine returns
    /// after the object test for `NO_ENEMY_OBJECT_OVERLAP` (BuildAssistant.cpp:766), so a `EnemiesOnly` call
    /// gets no structure-range or exit test and no terrain test unless `terrain` is set.
    pub fn is_location_legal_with(
        &self,
        t: &ObjectTemplate,
        pos: &Coord3D,
        angle: f32,
        options: LegalBuildOptions,
        builder_object: Option<PlayerIndex>,
        the_player: Option<PlayerIndex>,
    ) -> bool {
        let (w, h) = self.terrain.world_size();
        let diag = crate::skirmish_ai::l133_build_diag(&t.name);
        if diag {
            eprintln!(
                "ZR_L133_LAYER f={} tmpl={} pos=({:.4},{:.4}) layer={}",
                self.frame, t.name, pos.x, pos.y, self.terrain.layer_for_destination(pos)
            );
        }
        if pos.x < 0.0 || pos.y < 0.0 || pos.x > w || pos.y > h {
            if diag {
                eprintln!("ZR_L133_LEGAL f={} tmpl={} pos=({:.4},{:.4}) off_map", self.frame, t.name, pos.x, pos.y);
            }
            return false;
        }
        if options.terrain && !self.terrain_allows_build(t, pos, angle) {
            if diag {
                eprintln!("ZR_L133_LEGAL f={} tmpl={} pos=({:.4},{:.4}) terrain=0", self.frame, t.name, pos.x, pos.y);
            }
            return false;
        }
        let objects_ok = match options.objects {
            ObjectOverlap::Ignore => true,
            ObjectOverlap::All => self.is_location_clear_of_objects(t, pos, angle, builder_object, the_player, false),
            ObjectOverlap::EnemiesOnly => self.is_location_clear_of_objects(t, pos, angle, builder_object, the_player, true),
        };
        if diag {
            eprintln!(
                "ZR_L133_LEGAL f={} tmpl={} pos=({:.4},{:.4}) terrain={} objects={:?} objects_ok={}",
                self.frame, t.name, pos.x, pos.y, options.terrain, options.objects, objects_ok
            );
        }
        objects_ok
    }

    /// The `TERRAIN_RESTRICTIONS` block of `isLocationLegalToBuild` (`BuildAssistant.cpp:1027-1058`).
    fn terrain_allows_build(&self, t: &ObjectTemplate, pos: &Coord3D, angle: f32) -> bool {
        // `if (TheTerrainLogic->getLayerForDestination(worldPos) != LAYER_GROUND) return LBC_RESTRICTED_TERRAIN;`
        // (BuildAssistant.cpp:1036): a site whose position is on a bridge deck is restricted, before any sample.
        // `pos.z` decides the layer, so the caller's z is the one the engine tests.
        if self.terrain.layer_for_destination(pos) != crate::bridge::LAYER_GROUND {
            return false;
        }
        // TERRAIN_RESTRICTIONS (`BuildAssistant.cpp:1027-1058`): a quick footprint pass at three cells, then
        // a full pass at one cell. A sample on no cell, water, cliff or impassable ground restricts the site.
        // The height spread over the samples of both passes must stay within the allowed variation. Rubble
        // and obstacle cells do not restrict by terrain (the object test covers them). The
        // restrict-construction tile flag is not modelled (no terrain-type data in the port).
        let res = crate::pathfind::PATHFIND_CELL_SIZE_F;
        let mut hi = f32::MIN;
        let mut lo = f32::MAX;
        let diag = crate::skirmish_ai::l133_build_diag(&t.name);
        for sample_res in [3.0 * res, res] {
            let mut restricted = false;
            for p in self.footprint_samples(t, pos, angle, sample_res) {
                if let None | Some(CellType::Water | CellType::Cliff | CellType::Impassable) = self.pathfinder.cell_type_at(&p) {
                    restricted = true;
                }
                hi = hi.max(p.z);
                lo = lo.min(p.z);
            }
            if diag {
                let count = self.footprint_samples(t, pos, angle, sample_res).len();
                eprintln!(
                    "ZR_L133_TERRAIN f={} tmpl={} pos=({:.4},{:.4}) res={} samples={} restricted={} hi-lo={:.4} limit={:.4}",
                    self.frame, t.name, pos.x, pos.y, sample_res, count, restricted, hi - lo, self.rules.allowed_height_variation_for_building
                );
            }
            if restricted || hi - lo > self.rules.allowed_height_variation_for_building {
                return false;
            }
        }
        true
    }

    /// `BuildAssistant::isLocationClearOfObjects` (`BuildAssistant.cpp:656-907`, `NO_OBJECT_OVERLAP`), in two
    /// loops. Loop one takes the objects whose geometry collides with the footprint (`iteratePotentialCollisions`
    /// with `PartitionFilterWouldCollide`, `PartitionManager.cpp:3737`, `:5267`). Removable objects
    /// (`isRemovableForConstruction`), mines and inert objects are skipped. An ally that is using an ability or
    /// is busy blocks (`:720`), and so does any immobile object (`:729`, with no exception for scenery or
    /// non-structures), a disabled object (`:739`) and an enemy of the builder (`:754`).
    ///
    /// Loop two takes the structures in range (`:777`). Each one's body box, grown by its own
    /// `FactoryExtraBibWidth` and this site's `myExtraWidth`, must not collide with this footprint grown by
    /// `myExtraWidth` (`:842-863`). For an immobile structure, the factory exit rectangles also block: this
    /// site's against its body, its exit against this body, and the two exits against each other (`:874-903`).
    ///
    /// `builder_object` is the relationship source and `the_player` the skirmish-AI test (`:783`); see
    /// [`Self::is_location_legal_to_build_for`]. Deviations: the shroud codes are not returned (the port
    /// ignores shroud). `NO_ENEMY_OBJECT_OVERLAP` is [`ObjectOverlap::EnemiesOnly`]. Its supply-border test (`CANNOT_BUILD_NEAR_SUPPLIES`, BuildAssistant.cpp:967) is not modelled in either mode.
    fn is_location_clear_of_objects(
        &self,
        t: &ObjectTemplate,
        pos: &Coord3D,
        angle: f32,
        builder_object: Option<PlayerIndex>,
        the_player: Option<PlayerIndex>,
        enemy_only: bool,
    ) -> bool {
        use crate::partition::DistanceMode::FromBoundingSphere2D;
        use crate::template::kind_of;
        let g = &t.geometry;
        let footprint = Foot::of(pos, angle, g);
        let max_dist = g.bounding_sphere_radius() * 1.1;
        let diag = crate::skirmish_ai::l133_build_diag(&t.name);
        for id in self.partition.objects_in_range(pos, max_dist, FromBoundingSphere2D) {
            let Some(o) = self.objects.get(&id) else { continue };
            if !foot_collides(&footprint, &Foot::of(&o.position, o.angle, &o.template.geometry)) {
                continue;
            }
            if removable_for_construction(o) || o.is_kind_of(kind_of::MINE) || o.is_kind_of(kind_of::INERT) {
                continue;
            }
            let rel = builder_object.map_or(crate::damage::Relationship::Neutral, |b| self.relationship(b, o.player));
            if diag {
                eprintln!(
                    "ZR_L133_CLEAR f={} at=({:.4},{:.4}) against={} tmpl={} opos=({:.4},{:.4}) kindImmobile={} rel={:?} disabled={}",
                    self.frame, pos.x, pos.y, id.0, o.template.name, o.position.x, o.position.y,
                    o.is_kind_of(kind_of::IMMOBILE), rel, o.is_disabled()
                );
            }
            // `if (onlyCheckEnemies && builderObject && rel != ENEMIES) continue;` (BuildAssistant.cpp:712-715).
            if enemy_only && builder_object.is_some() && rel != crate::damage::Relationship::Enemies && o.is_kind_of(kind_of::IMMOBILE) {
                continue;
            }
            // "Prevent busy units (black lotus hacking from being moved by trying to place a building --
            // exploit)." (`BuildAssistant.cpp:719-728`): `them->getAI()->isBusy()` is
            // `getStateMachine()->isInBusyState()` (`AIUpdate.cpp:3139-3142`), i.e. the AI state machine's
            // current state is `AIBusyState` (`StateMachine.h:262`, the only `isBusy()` override,
            // `AIStateMachine.h:345`). It is not the hack-internet module's own state: that runs a separate
            // machine, so `AIUpdateInterface::update` never runs while it does.
            if rel == crate::damage::Relationship::Allies
                && (o.status_bits & crate::object::status::USING_ABILITY != 0
                    || o.ai.machine.current == crate::ai_states::AiStateType::Busy)
            {
                return false;
            }
            if o.is_kind_of(kind_of::IMMOBILE) || o.is_disabled() || rel == crate::damage::Relationship::Enemies {
                return false;
            }
        }

        // `if (onlyCheckEnemies) return LBC_OK;` (BuildAssistant.cpp:766): the enemy-only option stops here.
        if enemy_only {
            return true;
        }
        // `myFactoryExitWidth` and `myExtraWidth` (`:780-790`). A skirmish AI keeps 3 cells from other
        // structures, and when its bib is narrower it takes that margin out of its exit.
        let skirmish = the_player.is_some_and(|p| self.is_skirmish_ai_player(p));
        let mut my_exit = t.factory_exit_width;
        let mut my_extra = t.factory_extra_bib_width;
        if skirmish && my_extra < SKIRMISH_EXTRA_WIDTH {
            my_extra = SKIRMISH_EXTRA_WIDTH;
            my_exit = (my_exit - my_extra).max(0.0);
        }
        // `myBounds` (`:794-800`) and the exit rectangle (`myGeom`, `myExitPos`, `:801-814`).
        let mine = Foot::grown(pos, angle, g, my_extra, my_extra);
        let my_check_exit = my_exit > 0.0;
        let my_exit_foot = Foot::exit(&exit_centre(pos, angle, g.major_radius, my_exit), angle, g, my_exit);

        let range = 2.0 * (g.major_radius + g.minor_radius);
        for id in self.partition.objects_in_range(pos, range, FromBoundingSphere2D) {
            let Some(o) = self.objects.get(&id) else { continue };
            if !o.is_kind_of(kind_of::STRUCTURE) || removable_for_construction(o) {
                continue;
            }
            // `hisExtraWidth`, `hisBounds` (`:838-848`): a box grows its major by his bib and its minor by
            // `myExtraWidth` (the engine passes it here too); a non-box becomes a square of his major.
            let them = &o.template.geometry;
            let his_exit_width = o.template.factory_exit_width;
            let his = Foot::grown(&o.position, o.angle, them, o.template.factory_extra_bib_width, my_extra);
            if diag {
                eprintln!(
                    "ZR_L133_CLEAR2 f={} at=({:.4},{:.4}) against={} tmpl={} opos=({:.4},{:.4}) collides={} his={:?} major={} minor={} radius={} my_extra={} range={}",
                    self.frame, pos.x, pos.y, id.0, o.template.name, o.position.x, o.position.y,
                    foot_collides(&his, &mine), them.shape, them.major_radius, them.minor_radius,
                    them.bounding_circle_radius(), my_extra, range
                );
            }
            if foot_collides(&his, &mine) {
                return false;
            }
            // The exit checks (`:874-903`) apply only to an immobile structure.
            if !o.is_kind_of(kind_of::IMMOBILE) {
                continue;
            }
            let his_check_exit = his_exit_width > 0.0;
            if my_check_exit && foot_collides(&his, &my_exit_foot) {
                return false;
            }
            let his_exit = Foot::exit(&exit_centre(&o.position, o.angle, them.major_radius, his_exit_width), o.angle, them, his_exit_width);
            if his_check_exit && foot_collides(&his_exit, &mine) {
                return false;
            }
            if my_check_exit && his_check_exit && foot_collides(&his_exit, &my_exit_foot) {
                return false;
            }
        }
        true
    }

    /// `BuildAssistant::clearRemovableForConstruction` (`BuildAssistant.cpp:1394`): destroy the objects under the
    /// footprint of `t` at `pos` that `isRemovableForConstruction` clears (shrubbery, `CLEARED_BY_BUILD`, dead
    /// objects), except `ALWAYS_SELECTABLE` ones. The engine calls it from `buildObjectNow` (`:346`). Deviation:
    /// the engine also removes the trees and props from the terrain (`removeTreesAndPropsForConstruction`), which
    /// is client-side and not ported.
    pub(crate) fn clear_removable_for_construction(&mut self, t: &ObjectTemplate, pos: &Coord3D, angle: f32) {
        use crate::partition::DistanceMode::FromBoundingSphere2D;
        let footprint = Foot::of(pos, angle, &t.geometry);
        let max_dist = t.geometry.bounding_sphere_radius() * 1.1;
        let doomed: Vec<ObjectId> = self
            .partition
            .objects_in_range(pos, max_dist, FromBoundingSphere2D)
            .into_iter()
            .filter(|id| {
                self.objects.get(id).is_some_and(|o| {
                    removable_for_construction(o)
                        && !o.template.always_selectable
                        && foot_collides(&footprint, &Foot::of(&o.position, o.angle, &o.template.geometry))
                })
            })
            .collect();
        for id in doomed {
            self.destroy_object(id);
        }
    }

    /// `BuildAssistant::moveObjectsForConstruction` (`BuildAssistant.cpp:1421-1494`): move the units under the
    /// footprint of `t` at `pos` out of the way for `builder`'s construction. Returns `false` when an object
    /// could not be moved: an enemy, or a neutral or allied object with no AI interface (`:1486-1494`).
    ///
    /// The footprint is the `gi` box the engine builds (`:1426-1427`). A box template uses its own geometry;
    /// any other shape becomes a box of its major radius, 10 high. Mines, inert objects and removable or
    /// always-selectable objects are skipped (`:1444-1455`). Each movable unit gets
    /// `aiMoveToPositionEvenIfSleeping` (`:1483`) at `pos` plus a random distance and direction (`:1469-1474`):
    /// the distance is `GameLogicRandomValueReal(0.5, 1.5) * radius`, drawn before the angle
    /// `GameLogicRandomValueReal(-PI, PI)`, where `radius` is `1.4` times the corner distance of `gi`
    /// (`:1438-1439`).
    ///
    /// Deviations: the engine queries the partition with `use2D` false (`:1432-1435`, 3D bounding spheres). The
    /// port's partition has only the 2D mode, so the query is a superset, and the exact `PartitionFilterWouldCollide`
    /// test trims it. The engine's iterator is unsorted; the partition returns distance order with ties by id, so
    /// the draw order can differ when several units move. A disabled unit still uses its draws but is not ordered,
    /// as `Object::isMobile` is false for it (`Object.cpp:2952`) and `privateMoveToPosition` returns for a
    /// non-mobile unit (`AIUpdate.cpp:2891`). The port's `is_mobile` does not check disabled, so the check is local.
    /// Also called by `AISkirmishPlayer::newMap`'s `buildStructureNow` (`AIPlayer.cpp:456`).
    pub(crate) fn move_objects_for_construction(&mut self, t: &ObjectTemplate, pos: &Coord3D, angle: f32, builder: PlayerIndex) -> bool {
        use crate::ai_states::CommandSource;
        use crate::damage::Relationship;
        use crate::partition::DistanceMode::FromBoundingSphere2D;
        use crate::template::{kind_of, Geometry, GeometryShape};
        let gi = if t.geometry.shape == GeometryShape::Box {
            t.geometry
        } else {
            Geometry { shape: GeometryShape::Box, major_radius: t.geometry.major_radius, minor_radius: t.geometry.major_radius, height: 10.0, is_small: false }
        };
        let footprint = Foot::of(pos, angle, &gi);
        let max_dist = gi.bounding_sphere_radius() * 1.1;
        // `radius` (`:1431-1433`): the corner distance in double, as the engine's `sqrt(pow(..))`, then `* 1.4f`.
        let (major, minor) = (gi.major_radius as f64, gi.minor_radius as f64);
        let radius = (major * major + minor * minor).sqrt() as f32 * 1.4;
        let mut all_moved = true;
        for id in self.partition.objects_in_range(pos, max_dist, FromBoundingSphere2D) {
            let Some(o) = self.objects.get(&id) else { continue };
            if !foot_collides(&footprint, &Foot::of(&o.position, o.angle, &o.template.geometry)) {
                continue;
            }
            // `:1383-1396`: mines and inert objects are built over; removable and always-selectable ones are skipped.
            if o.is_kind_of(kind_of::MINE) || o.is_kind_of(kind_of::INERT) || o.template.always_selectable || removable_for_construction(o) {
                continue;
            }
            let rel = self.relationship(builder, o.player);
            let neutral_or_allied = matches!(rel, Relationship::Neutral | Relationship::Allies);
            let has_ai = o.ai_handle.is_some();
            let disabled = o.is_disabled();
            if !neutral_or_allied || !has_ai {
                all_moved = false;
                continue;
            }
            // `:1459-1466`: the distance draw, then the direction draw, then `vec.Rotate_Z(dir)` on `(r, 0, 0)`.
            let varied = self.rng.value_real(0.5, 1.5) * radius;
            let dir = self.rng.value_real(-crate::math::PI, crate::math::PI);
            let dest = Coord3D::new(pos.x + varied * cos(dir), pos.y + varied * sin(dir), pos.z);
            if !disabled {
                // `:1483`, the `CMD_FROM_AI` move that ignores the sleep mood (`AIUpdate.cpp:2603`).
                self.ai_private_move_to_position(id, dest, CommandSource::Ai);
            }
        }
        all_moved
    }
}

/// `BuildAssistant.cpp:786`: `3 * PATHFIND_CELL_SIZE_F`, the margin a skirmish AI keeps around its builds.
const SKIRMISH_EXTRA_WIDTH: f32 = 3.0 * crate::pathfind::PATHFIND_CELL_SIZE_F;

/// The centre of a factory exit rectangle: `offset = major + exit / 2` along `angle` from `pos`, at `pos`'s
/// height (`BuildAssistant.cpp:809-813`, `:859-861`).
fn exit_centre(pos: &Coord3D, angle: f32, body_major: f32, exit_width: f32) -> Coord3D {
    let offset = body_major + exit_width / 2.0;
    Coord3D::new(pos.x + crate::math::cos(angle) * offset, pos.y + crate::math::sin(angle) * offset, pos.z)
}

/// A collision shape for the build tests (`GeometryInfo` / `CollideInfo`): an oriented box with
/// `major`/`minor` half extents, a cylinder of radius `major`/`minor`, or a sphere of radius `major`.
/// `height` is `GeometryInfo::getMaxHeightAbovePosition` (a box's or cylinder's height, a sphere's
/// major radius).
#[derive(Clone, Copy)]
struct Foot {
    pos: Coord3D,
    angle: f32,
    boxed: bool,
    /// `GeometryInfo::getGeomType() == GEOMETRY_SPHERE`; it changes the z test and the rect test's
    /// minor radius (`PartitionManager.cpp:413-414`, `:624-648`).
    sphere: bool,
    major: f32,
    minor: f32,
    height: f32,
}

impl Foot {
    /// A template or object geometry as it stands. The height is `GeometryInfo::getMaxHeightAbovePosition`
    /// (`Geometry.cpp:210`): a sphere's is its major radius, a box's or cylinder's is its height.
    fn of(pos: &Coord3D, angle: f32, g: &crate::template::Geometry) -> Foot {
        use crate::template::GeometryShape;
        Foot {
            pos: *pos,
            angle,
            boxed: g.shape == GeometryShape::Box,
            sphere: g.shape == GeometryShape::Sphere,
            major: g.major_radius,
            minor: g.minor_radius,
            height: if g.shape == GeometryShape::Sphere { g.major_radius } else { g.height },
        }
    }

    /// The factory exit rectangle of `g` at `pos` (`myGeom` / `hisGeom`, `BuildAssistant.cpp:801-812` and
    /// `:850-861`): the major radius becomes half the exit width. A box keeps its shape and minor radius. Any
    /// other shape keeps its type, so the circle test reads the half width. Its height follows the same rule
    /// as [`Foot::of`].
    fn exit(pos: &Coord3D, angle: f32, g: &crate::template::Geometry, exit_width: f32) -> Foot {
        use crate::template::GeometryShape;
        let half = exit_width / 2.0;
        let boxed = g.shape == GeometryShape::Box;
        Foot {
            pos: *pos,
            angle,
            boxed,
            sphere: g.shape == GeometryShape::Sphere,
            major: half,
            minor: if boxed { g.minor_radius } else { g.major_radius },
            height: if g.shape == GeometryShape::Sphere { half } else { g.height },
        }
    }

    /// `GeometryInfo` after the build's extra width. A box grows its major and minor radii; any other
    /// shape becomes a square of side `major` and 40 high (`GeometryInfo::set(GEOMETRY_BOX, false, 40, ..)`).
    fn grown(pos: &Coord3D, angle: f32, g: &crate::template::Geometry, major_plus: f32, minor_plus: f32) -> Foot {
        if g.shape == crate::template::GeometryShape::Box {
            Foot {
                pos: *pos,
                angle,
                boxed: true,
                sphere: false,
                major: g.major_radius + major_plus,
                minor: g.minor_radius + minor_plus,
                height: g.height,
            }
        } else {
            let m = g.major_radius + major_plus;
            Foot { pos: *pos, angle, boxed: true, sphere: false, major: m, minor: m, height: 40.0 }
        }
    }
}

/// `PartitionFilterWouldCollide::allow` (`PartitionManager.cpp:5267-5300`): the z-interval test, then
/// the geometry pair's collide proc (`theCollideTestProcs`, `:965-976`). The pair procs are
/// `collideTest_Sphere_Sphere` (3D distance), `z_collideTest_Sphere_Nonsphere` (box/cylinder) and
/// `z_collideTest_Nonsphere_Nonsphere` (the non-sphere kinds; the caller has already done the z test).
fn foot_collides(a: &Foot, b: &Foot) -> bool {
    if a.pos.z + a.height < b.pos.z || a.pos.z > b.pos.z + b.height {
        return false;
    }
    match (a.sphere, b.sphere) {
        (true, true) => foot_sphere_sphere(a, b),
        (true, false) => foot_sphere_nonsphere(a, b),
        (false, true) => foot_sphere_nonsphere(b, a),
        (false, false) => foot_nonsphere_nonsphere(a, b),
    }
}

/// `collideTest_Sphere_Sphere` (`PartitionManager.cpp:699-724`): a **3D** centre distance against the
/// two radii.
fn foot_sphere_sphere(a: &Foot, b: &Foot) -> bool {
    let dx = b.pos.x - a.pos.x;
    let dy = b.pos.y - a.pos.y;
    let dz = b.pos.z - a.pos.z;
    let r = a.major + b.major;
    dx * dx + dy * dy + dz * dz <= r * r
}

/// `z_collideTest_Sphere_Nonsphere` (`PartitionManager.cpp:624-681`): when the sphere's centre is
/// inside the other shape's z range, the plain 2D test; when it is below or above, the sphere's radius
/// is shrunk to the slice at that plane. A sphere uses `xy_collideTest_Circle_Rect` (the circle is a
/// square in the compiled test), a cylinder `xy_collideTest_Circle_Circle`.
fn foot_sphere_nonsphere(s: &Foot, n: &Foot) -> bool {
    let n_top = n.pos.z + n.height;
    let xy = |s: &Foot, n: &Foot| -> bool {
        if n.boxed {
            // `xy_collideTest_Rect_Circle` / `xy_collideTest_Circle_Rect`
            // (`PartitionManager.cpp:479-486`): the circle is given its major radius as its minor
            // radius and tested as a rectangle.
            foot_rect_rect(n, &Foot { minor: s.major, boxed: true, ..*s })
        } else {
            let (dx, dy) = (n.pos.x - s.pos.x, n.pos.y - s.pos.y);
            let r = s.major + n.major;
            dx * dx + dy * dy <= r * r
        }
    };
    if s.pos.z >= n.pos.z && s.pos.z <= n_top {
        return xy(s, n);
    }
    if s.pos.z < n.pos.z && s.pos.z + s.major >= n.pos.z {
        let mut sa = *s;
        sa.pos.z = n.pos.z;
        sa.major = crate::math::sqrt(s.major * s.major - (n.pos.z - s.pos.z) * (n.pos.z - s.pos.z));
        return xy(&sa, n);
    }
    if s.pos.z > n_top && s.pos.z - s.major <= n_top {
        let mut sa = *s;
        sa.pos.z = n_top;
        sa.major = crate::math::sqrt(s.major * s.major - (s.pos.z - n_top) * (s.pos.z - n_top));
        return xy(&sa, n);
    }
    false
}

/// `z_collideTest_Nonsphere_Nonsphere` (`PartitionManager.cpp:683-696`): the caller's z-interval test
/// has passed. A centre distance below the smallest of the four half extents counts as a hit before
/// the 2D test; otherwise the pair's 2D proc decides (rect/rect with the corner test for two boxes,
/// rect/circle when one is a cylinder).
fn foot_nonsphere_nonsphere(a: &Foot, b: &Foot) -> bool {
    let d_sqr = (a.pos.x - b.pos.x) * (a.pos.x - b.pos.x) + (a.pos.y - b.pos.y) * (a.pos.y - b.pos.y);
    let mut min_radius = a.major.min(a.minor);
    min_radius = min_radius.min(b.major).min(b.minor);
    if min_radius * min_radius > d_sqr {
        return true;
    }
    match (a.boxed, b.boxed) {
        (true, true) => foot_rect_rect(a, b),
        (true, false) => foot_rect_circle(a, b),
        (false, true) => foot_rect_circle(b, a),
        (false, false) => {
            let r = a.major + b.major;
            d_sqr <= r * r
        }
    }
}

/// `xy_collideTest_Rect_Circle` (`PartitionManager.cpp:479-486`): the compiled branch gives the circle
/// its major radius as its minor radius and runs [`foot_rect_rect`] (a square, not a true circle).
fn foot_rect_circle(r: &Foot, c: &Foot) -> bool {
    foot_rect_rect(r, &Foot { minor: c.major, boxed: true, ..*c })
}

/// `xy_collideTest_Rect_Rect` (`PartitionManager.cpp:552-601`): true when one rectangle's corner lies
/// inside the other. This is the engine's "not as accurate as it could be" test (`rectToFourPoints`
/// and `testRotatedPointsAgainstRect`), not a separating-axis test. A sphere's minor radius is its
/// major (`:413-414`).
fn foot_rect_rect(a: &Foot, b: &Foot) -> bool {
    /// `rectToFourPoints` (`PartitionManager.cpp:433-466`): tl, tr, bl, br.
    fn corners(f: &Foot) -> [(f32, f32); 4] {
        let c = crate::math::cos(f.angle);
        let s = crate::math::sin(f.angle);
        let exc = f.major * c;
        let eyc = f.minor * c;
        let exs = f.major * s;
        let eys = f.minor * s;
        let (x, y) = (f.pos.x, f.pos.y);
        [
            (x - exc - eys, y + eyc - exs),
            (x + exc - eys, y + eyc + exs),
            (x - exc + eys, y - eyc - exs),
            (x + exc + eys, y - eyc + exs),
        ]
    }
    /// `testRotatedPointsAgainstRect` (`:398-431`): one point of the other rect inside `r`.
    fn any_corner_inside(pts: &[(f32, f32); 4], r: &Foot) -> bool {
        let c = crate::math::cos(-r.angle);
        let s = crate::math::sin(-r.angle);
        let minor = if r.sphere { r.major } else { r.minor };
        pts.iter().any(|&(px, py)| {
            let ptx = px - r.pos.x;
            let pty = py - r.pos.y;
            let nx = (ptx * c - pty * s).abs();
            let ny = (ptx * s + pty * c).abs();
            nx <= r.major && ny <= minor
        })
    }
    any_corner_inside(&corners(a), b) || any_corner_inside(&corners(b), a)
}

/// `BuildAssistant::isRemovableForConstruction` (`BuildAssistant.cpp:1360-1388`): `KINDOF_INERT` is never
/// removable (`:1367`), then shrubbery (`:1372`), `KINDOF_CLEARED_BY_BUILD` (`:1376`) and anything
/// `isEffectivelyDead()` (`:1381-1382`) are. The engine's `EFFECTIVELY_DEAD` is the object flag
/// `Object::m_privateStatus` (`Object.cpp:2041-2054`), set by the die path and, for a template with no
/// `ActiveBody`, by the `InactiveBody` constructor (`InactiveBody.cpp:47-50`) — so a bodyless object is
/// removable without a special case here.
fn removable_for_construction(o: &crate::object::Object) -> bool {
    use crate::template::kind_of;
    if o.is_kind_of(kind_of::INERT) {
        return false;
    }
    o.is_kind_of(kind_of::SHRUBBERY) || o.template.cleared_by_build || o.destroyed || o.is_dying()
}

/// A point on the line from the site to the dozer, just outside the building's footprint.
pub(crate) fn approach_point(site: &Coord3D, from: &Coord3D, distance: f32) -> Coord3D {
    let (dx, dy) = (from.x - site.x, from.y - site.y);
    let d = (dx * dx + dy * dy).sqrt();
    if d <= 0.001 {
        return Coord3D::new(site.x + distance, site.y, site.z);
    }
    Coord3D::new(site.x + dx / d * distance, site.y + dy / d * distance, site.z)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::{Geometry, GeometryShape};

    fn geom(shape: GeometryShape, major: f32, minor: f32) -> Geometry {
        Geometry { shape, major_radius: major, minor_radius: minor, height: 20.0, is_small: false }
    }

    /// `isLocationClearOfObjects` loop two: a skirmish AI's footprint (grown by `myExtraWidth` = 30)
    /// collides with a structure 25 units away that a neutral footprint clears.
    #[test]
    fn skirmish_margin_keeps_thirty_units_from_a_box() {
        let other = Foot::grown(&Coord3D::new(0.0, 0.0, 0.0), 0.0, &geom(GeometryShape::Box, 20.0, 20.0), 0.0, 0.0);
        let site = geom(GeometryShape::Box, 20.0, 20.0);
        let at = Coord3D::new(65.0, 0.0, 0.0);
        let neutral = Foot::grown(&at, 0.0, &site, 0.0, 0.0);
        let skirmish = Foot::grown(&at, 0.0, &site, 30.0, 30.0);
        assert!(!foot_collides(&other, &neutral));
        assert!(foot_collides(&other, &skirmish));
    }

    /// Circles collide when their centres are closer than the sum of the radii; a non-box shape grows to
    /// a square (`GeometryInfo::set(GEOMETRY_BOX, ...)`), so a 10-radius circle covers `[-10, 10]` on each axis.
    #[test]
    fn circle_footprints_use_their_radii() {
        let a = Foot::of(&Coord3D::new(0.0, 0.0, 0.0), 0.0, &geom(GeometryShape::Sphere, 10.0, 10.0));
        let near = Foot::of(&Coord3D::new(19.0, 0.0, 0.0), 0.0, &geom(GeometryShape::Sphere, 10.0, 10.0));
        let far = Foot::of(&Coord3D::new(21.0, 0.0, 0.0), 0.0, &geom(GeometryShape::Sphere, 10.0, 10.0));
        assert!(foot_collides(&a, &near));
        assert!(!foot_collides(&a, &far));
        let square = Foot::grown(&Coord3D::new(0.0, 0.0, 0.0), 0.0, &geom(GeometryShape::Sphere, 10.0, 10.0), 0.0, 0.0);
        assert!(square.boxed && square.major == 10.0 && square.height == 40.0);
    }

    /// `GeometryInfo::getMaxHeightAbovePosition` (`Geometry.cpp:214-220`): a sphere's height is its major
    /// radius, a box's is its height. A sphere 10 high with radius 3 reaches 3 above its position.
    #[test]
    fn height_above_position_follows_the_shape() {
        let p = Coord3D::new(0.0, 0.0, 0.0);
        let sphere = Foot::of(&p, 0.0, &Geometry { shape: GeometryShape::Sphere, major_radius: 3.0, minor_radius: 3.0, height: 10.0, is_small: false });
        let boxed = Foot::of(&p, 0.0, &geom(GeometryShape::Box, 4.0, 4.0));
        assert_eq!(sphere.height, 3.0);
        assert_eq!(boxed.height, 20.0);
    }

    /// `BuildAssistant::isRemovableForConstruction` with a bodyless object: `InactiveBody`'s constructor
    /// sets `EFFECTIVELY_DEAD` (`InactiveBody.cpp:47-50`), so a map's ambient sound marker (no body) under a
    /// building's footprint is destroyed when construction starts, while an object with a body is not.
    /// `clearRemovableForConstruction` runs from `BuildAssistant::buildObjectNow` (`:346`) and `construct`.
    #[test]
    fn construction_clears_a_bodyless_object_under_its_footprint() {
        use crate::template::{kind_of, ObjectTemplate};
        let mut g = GameLogic::new(crate::TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 0);
        let mut marker = ObjectTemplate::new("AmbientMarker");
        marker.kind_of = kind_of::IMMOBILE;
        // No `Body` module in the INI: `Object` gives it an `InactiveBody`.
        marker.max_health = 0.0;
        marker.initial_health = 0.0;
        let marker = std::sync::Arc::new(marker);
        let mut rock = ObjectTemplate::new("Rock");
        rock.kind_of = kind_of::IMMOBILE;
        rock.max_health = 100.0;
        rock.initial_health = 100.0;
        rock.geometry = geom(GeometryShape::Sphere, 5.0, 5.0);
        let rock = std::sync::Arc::new(rock);
        let mut house = ObjectTemplate::new("House");
        house.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
        house.geometry = geom(GeometryShape::Box, 30.0, 30.0);
        let at = Coord3D::new(100.0, 100.0, 0.0);
        let marker_id = g.create_object(&marker, at, 0.0, 0);
        let rock_id = g.create_object(&rock, Coord3D::new(105.0, 100.0, 0.0), 0.0, 0);
        g.clear_removable_for_construction(&house, &at, 0.0);
        assert!(g.find_object(marker_id).is_none(), "the bodyless marker is effectively dead and removable");
        assert!(g.find_object(rock_id).is_some(), "an object with a body is not removable");
    }

    /// A template without an `ActiveBody` gets the default `Body = InactiveBody` (`data/ini/default/object.ini:89`),
    /// whose constructor sets `EFFECTIVELY_DEAD` (`InactiveBody.cpp:47-50`). The port models the engine's
    /// `Object::m_privateStatus` bit as `Object::effectively_dead`, set at creation, so every `isEffectivelyDead()`
    /// reader sees a bodyless object as dead (mood/acquire, targeting, `isRemovableForConstruction`).
    #[test]
    fn a_bodyless_object_is_effectively_dead_from_creation() {
        use crate::template::{kind_of, ObjectTemplate};
        let mut g = GameLogic::new(crate::TerrainLogic::flat(100, 100, 0), 1);
        let mut marker = ObjectTemplate::new("AmbientMarker");
        marker.kind_of = kind_of::IMMOBILE;
        marker.max_health = 0.0;
        marker.initial_health = 0.0;
        let mut rock = ObjectTemplate::new("Rock");
        rock.kind_of = kind_of::IMMOBILE;
        rock.max_health = 100.0;
        rock.initial_health = 100.0;
        let at = Coord3D::new(10.0, 10.0, 0.0);
        let marker_id = g.create_object(&std::sync::Arc::new(marker), at, 0.0, 0);
        let rock_id = g.create_object(&std::sync::Arc::new(rock), Coord3D::new(30.0, 10.0, 0.0), 0.0, 0);
        assert!(g.is_effectively_dead(marker_id), "InactiveBody sets EFFECTIVELY_DEAD");
        assert!(!g.is_effectively_dead(rock_id), "an ActiveBody object is alive");
        // `Object::is_dying` is the port's combined "dead or effectively dead" test that the mood scan,
        // targeting and the construction clearing read.
        assert!(g.find_object(marker_id).unwrap().is_dying());
        assert!(!g.find_object(rock_id).unwrap().is_dying());
    }

    /// `Player::onStructureConstructionComplete` (`Player.cpp:1640-1641`) re-classifies the finished
    /// structure's footprint (`removeObjectFromPathfindMap` then `addObjectToPathfindMap`). Each arm
    /// marks the touched cells `UNINITIALIZED_ZONE` (`AIPathfind.cpp:4500-4518`), and the re-add's
    /// `updateZonesForModify` cannot give an obstacle footprint's cells a zone from a same-type
    /// neighbour that has one, so the cells stay 0 until the next scheduled `calculateZones`. This is
    /// what the C++ build shows for the GLA stash that completes at f1121 (lane 115). The test
    /// re-zones the grid while the site is under construction (giving its cells a real obstacle
    /// zone) and then lets the dozer finish.
    #[test]
    fn construction_completion_resets_the_footprint_cells_zone() {
        use crate::template::{kind_of, GeometryShape, LocomotorKind, LocomotorParams};
        let mut dozer = ObjectTemplate::new("Dozer");
        dozer.max_health = 100.0;
        dozer.initial_health = 100.0;
        dozer.geometry.major_radius = 5.0;
        dozer.geometry.minor_radius = 5.0;
        dozer.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
        dozer.kind_of |= kind_of::DOZER;
        dozer.dozer = Some(crate::dozer::DozerTemplate { repair_health_percent_per_second: 0.0, bored_time_frames: 1.0, bored_range: 0.0 });
        let mut site = ObjectTemplate::new("Barracks");
        site.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
        site.max_health = 500.0;
        site.initial_health = 500.0;
        site.geometry = geom(GeometryShape::Box, 15.0, 15.0);
        site.build_cost = 300;
        site.build_time_frames = 30;
        let mut store = crate::template::SimpleTemplateStore::default();
        store.add(dozer);
        store.add(site);
        let mut g = GameLogic::new(crate::TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        g.set_template_source(std::sync::Arc::new(store));
        let t = g.find_template("Dozer").expect("dozer template");
        let d = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let at = Coord3D::new(150.0, 100.0, 0.0);
        let b = g.construct(d, "Barracks", at, 0.0).expect("the site is created");
        // Walk the dozer over and wait for the first construction tick.
        for _ in 0..400 {
            g.run_frames(1);
            if g.find_object(b).is_some_and(|o| o.construction_percent > 0.0) {
                break;
            }
        }
        let o = g.find_object(b).expect("building");
        assert!(o.under_construction && o.construction_percent > 0.0, "the dozer is building: pct {}", o.construction_percent);
        let centre = g.find_object(b).unwrap().position;
        assert_eq!(g.pathfinder.cell_type_at(&centre), Some(CellType::Obstacle));
        // A zone calculation while under construction gives the footprint a real obstacle zone.
        g.pathfinder.calculate_zones();
        let before = g.pathfinder.cell_zone_at(&centre).expect("on the grid");
        assert_ne!(before, 0, "a calculated obstacle cell has a zone");
        // Finish construction; the completion re-classification must reset the cells.
        let mut done = false;
        for _ in 0..200 {
            g.run_frames(1);
            if g.find_object(b).is_some_and(|o| !o.under_construction) {
                done = true;
                break;
            }
        }
        assert!(done, "the site never finished");
        assert_eq!(g.pathfinder.cell_zone_at(&centre), Some(0), "the completed footprint is UNINITIALIZED_ZONE");
        assert_eq!(g.pathfinder.cell_type_at(&centre), Some(CellType::Obstacle), "it is still an obstacle");
    }

    /// `xy_collideTest_Rect_Circle` (`PartitionManager.cpp:479-486`, the compiled `#if 1` branch)
    /// squares the circle (`setMinorRadius(getMajorRadius())`) and runs the corner-in-rect test, so a
    /// small sphere can "collide" with a box even when no point of the true circle touches it: a
    /// radius-5 sphere at (514,514) has its square's (509,509) corner inside a 20×20 box centred at
    /// (500,500), while the closest true circle point is 5.657 away. The engine's ring search depends
    /// on this (worker 239's dock approach at f1307).
    #[test]
    fn a_small_sphere_collides_through_its_bounding_square() {
        use crate::template::{kind_of, ObjectTemplate};
        let mut g = GameLogic::new(crate::TerrainLogic::flat(600, 600, 0), 1);
        g.add_player(0, 0);
        let mut house = ObjectTemplate::new("House");
        house.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
        house.max_health = 500.0;
        house.initial_health = 500.0;
        house.geometry = geom(GeometryShape::Box, 10.0, 10.0);
        g.create_object(&std::sync::Arc::new(house), Coord3D::new(500.0, 500.0, 0.0), 0.0, 0);
        // The squared circle's corner is inside the box while no point of the true circle reaches it
        // (distance to the corner 5.23 > 5); the partition query still includes the box
        // (`FROM_BOUNDINGSPHERE_2D`: 19.37 - 14.14 = 5.23 < 5.5).
        assert!(!g.small_sphere_clear_at(&Coord3D::new(513.7, 513.7, 0.0), 0.0, &[]));
        // A square corner that just misses the box: both tests agree it is clear.
        assert!(g.small_sphere_clear_at(&Coord3D::new(520.0, 520.0, 0.0), 0.0, &[]));
    }

    /// `tryPosition` passes the ring sample's angle to `iteratePotentialCollisions`
    /// (`PartitionManager.cpp:3880`, `:3723-3743`), and the engine's collision record turns the query
    /// sphere with it (`CollideInfo(&m_position, m_geom, m_angle)`). Since `xy_collideTest_Rect_Circle`
    /// squares the sphere (`:479-486`), a rotated sample changes whether a corner falls inside a box:
    /// the AN3 f1303 sample at (1688.5240,2232.1497) is clear of the GLA supply stash's 72x70 box at
    /// (1655.3507,2206.3503), angle -0.7854, when the 5-radius square is turned by the ring angle
    /// 0.841568, but its *unturned* square's corner (1683.52,2227.15) lies inside the box.
    #[test]
    fn a_ring_sample_squares_the_sphere_in_the_sample_direction() {
        use crate::template::{kind_of, ObjectTemplate};
        let mut g = GameLogic::new(crate::TerrainLogic::flat(1800, 2400, 0), 1);
        g.add_player(0, 0);
        let mut stash = ObjectTemplate::new("Stash");
        stash.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
        stash.max_health = 1000.0;
        stash.initial_health = 1000.0;
        stash.geometry = geom(GeometryShape::Box, 36.0, 35.0);
        g.create_object(&std::sync::Arc::new(stash), Coord3D::new(1655.3507, 2206.3503, 18.75), -std::f32::consts::FRAC_PI_4, 0);
        let p = Coord3D::new(1688.524, 2232.1497, 18.75);
        assert!(g.small_sphere_clear_at(&p, 0.841568, &[]), "the square turned by the ring angle misses the box");
        assert!(!g.small_sphere_clear_at(&p, 0.0, &[]), "an unturned square's corner reaches inside the box");
    }

    /// `collideTest_Sphere_Sphere` (`PartitionManager.cpp:699-724`) compares the **3D** centre
    /// distance with the two radii once the z intervals overlap: a radius-5 sphere at (309,300,3)
    /// has its centre 9 units in 2D and 5 in z from the one at (300,300,8), so the 3D distance 10.296
    /// is beyond the 10 the two radii cover, while the older 2D distance test accepted it.
    #[test]
    fn two_small_spheres_collide_through_the_third_dimension() {
        use crate::template::{kind_of, ObjectTemplate};
        let mut g = GameLogic::new(crate::TerrainLogic::flat(600, 600, 0), 1);
        g.add_player(0, 0);
        let mut ball = ObjectTemplate::new("Ball");
        ball.kind_of = kind_of::IMMOBILE;
        ball.max_health = 100.0;
        ball.initial_health = 100.0;
        ball.geometry = geom(GeometryShape::Sphere, 5.0, 5.0);
        g.create_object(&std::sync::Arc::new(ball), Coord3D::new(300.0, 300.0, 8.0), 0.0, 0);
        // z intervals overlap (3..8 and 8..13); 2D 9, 3D 10.296 > 10 -> no hit.
        assert!(g.small_sphere_clear_at(&Coord3D::new(309.0, 300.0, 3.0), 0.0, &[]), "3D distance 10.296 > 10");
        // 2D 6, 3D 7.81 <= 10 -> hit.
        assert!(!g.small_sphere_clear_at(&Coord3D::new(306.0, 300.0, 3.0), 0.0, &[]), "3D distance 7.81 < 10");
    }

    /// A factory exit is a rectangle of half-width `exit / 2`, centred `major + exit / 2` in front of the body
    /// (`BuildAssistant.cpp:809-813`). A box keeps its minor radius; a round body's exit stays a circle of the
    /// half width, so its radius is 20 for a 40-wide exit.
    #[test]
    fn exit_rectangle_sits_in_front_of_the_body() {
        let c = exit_centre(&Coord3D::new(100.0, 50.0, 7.0), 0.0, 15.0, 40.0);
        assert_eq!((c.x, c.y, c.z), (135.0, 50.0, 7.0));
        let boxed = Foot::exit(&c, 0.0, &geom(GeometryShape::Box, 15.0, 12.0), 40.0);
        assert!(boxed.boxed && boxed.major == 20.0 && boxed.minor == 12.0 && boxed.height == 20.0);
        let round = Foot::exit(&c, 0.0, &geom(GeometryShape::Sphere, 15.0, 15.0), 40.0);
        assert!(!round.boxed && round.major == 20.0 && round.height == 20.0);
        // An exit of zero width is a zero-radius shape, so `is_location_clear_of_objects` never checks it.
        assert_eq!(exit_centre(&Coord3D::new(0.0, 0.0, 0.0), 0.0, 15.0, 0.0).x, 15.0);
    }
}
