//! `HordeUpdate` (GameLogic/Object/Update/HordeUpdate.cpp): the horde test that gives the HORDE (and, through
//! `AIUpdateInterface::evaluateMoraleBonus`, NATIONALISM / FANATICISM) weapon bonus. A sleepy-heap family
//! ([`FamilyKind::Horde`](crate::sleepy_families)): its constructor sets the first wake from a random phase
//! (HordeUpdate.cpp:161-169) and `update` returns `UPDATE_SLEEP(UpdateRate)` for infantry, `UPDATE_SLEEP_NONE` for
//! the rest (:377). The client decal and flag code is not simulated.

use crate::damage::Relationship;
use crate::logic::GameLogic;
use crate::modules2::HordeTemplate;
use crate::module::{UPDATE_SLEEP_FOREVER, UPDATE_SLEEP_NONE};
use crate::object::{Object, ObjectId};
use crate::template::{kind_of, Geometry, GeometryShape};

/// `GeometryInfo::getZDeltaToCenterPosition` (Geometry.cpp:245-255): a sphere's centre is its position, any other
/// shape's is half its height above it.
fn z_delta_to_centre(g: &Geometry) -> f32 {
    match g.shape {
        GeometryShape::Sphere => 0.0,
        _ => g.height * 0.5,
    }
}

/// `distCalcProc_BoundaryAndBoundary_3D` (PartitionManager.cpp:890-935), the distance `FROM_BOUNDINGSPHERE_3D`:
/// the centre-to-centre 3D distance (the z of each centre includes its geometry's `getZDeltaToCenterPosition`)
/// shrunk by both bounding-sphere radii, floored at zero, squared. Returned squared, as the C++ `abDistSqr`.
fn bounding_sphere_distance_sq(a: &Object, b: &Object) -> f32 {
    let (ga, gb) = (&a.template.geometry, &b.template.geometry);
    let dx = b.position.x - a.position.x;
    let dy = b.position.y - a.position.y;
    let dz = (b.position.z + z_delta_to_centre(gb)) - (a.position.z + z_delta_to_centre(ga));
    let actual_sq = dx * dx + dy * dy + dz * dz;
    let total_rad = crate::combat::geometry_bounding_sphere_radius(ga) + crate::combat::geometry_bounding_sphere_radius(gb);
    if total_rad > 0.0 {
        let shrunken = actual_sq.sqrt() - total_rad;
        if shrunken <= 0.0 {
            0.0
        } else {
            shrunken * shrunken
        }
    } else {
        actual_sq
    }
}

/// `distCalcProc_CenterAndCenter_2D` (PartitionManager.cpp:775-800), `FROM_CENTER_2D`: no shrink, no z.
fn center_distance_2d_sq(a: &Object, b: &Object) -> f32 {
    let dx = b.position.x - a.position.x;
    let dy = b.position.y - a.position.y;
    dx * dx + dy * dy
}

impl GameLogic {
    /// `HordeUpdate::HordeUpdate` (HordeUpdate.cpp:161-169): the first wake is `UPDATE_SLEEP(GameLogicRandomValue(1,
    /// UpdateRate))` from now. The draw is made by `create_object_full`'s constructor pass (`CtorDraw::Horde`), in the
    /// template's module order; this returns the wake frame it gives.
    pub(crate) fn horde_constructor_wake(&mut self, update_rate: u32, now: u32) -> u32 {
        let delay = update_rate.clamp(1, i32::MAX as u32) as i32;
        now.saturating_add(self.rng.value(1, delay) as u32)
    }

    /// The `PartitionFilterHordeMember` scan (HordeUpdate.cpp:81-111) over `iterateObjectsInRange(obj, m_minDist,
    /// FROM_BOUNDINGSPHERE_3D)` (HordeUpdate.cpp:267): the objects in the partition's cells that pass the filter and
    /// lie within `Radius` by [`bounding_sphere_distance_sq`]. The dead-unit exclusion is a `!RETAIL_COMPATIBLE_CRC`
    /// bugfix (HordeUpdate.cpp:83-87) and is off in the retail build the C++ reference is compiled as. Off-map is not
    /// modelled (the port has no `OFF_MAP` status).
    fn horde_members(&self, id: ObjectId, t: &HordeTemplate) -> Vec<ObjectId> {
        let Some(me) = self.objects.get(&id) else { return Vec::new() };
        let max_dist_sq = t.radius * t.radius;
        let mut out = Vec::new();
        for cid in self.partition.gco_candidates(&me.position, t.radius) {
            if cid == id {
                continue;
            }
            let Some(other) = self.objects.get(&cid) else { continue };
            // `m_exactMatch && m_obj->getTemplate() != objOther->getTemplate()` (:90).
            if t.exact_match && other.template.name != me.template.name {
                continue;
            }
            // `getHUI(objOther)`: only a `HordeUpdate` object can join (:93-96).
            if other.template.horde.is_none() {
                continue;
            }
            // `isKindOfMulti(m_kindof, KINDOFMASK_NONE)` (:99): every bit of the mask, nothing excluded.
            if t.kind_of != 0 && !other.template.is_kind_of(t.kind_of) {
                continue;
            }
            // `m_alliesOnly && getRelationship(objOther) != ALLIES` (:103).
            if t.allies_only && self.relationship(me.player, other.player) != Relationship::Allies {
                continue;
            }
            // `getClosestObjects`: `distProc(...) < closestDistSqr` with `closestDistSqr = maxDist * maxDist`.
            if bounding_sphere_distance_sq(me, other) < max_dist_sq {
                out.push(cid);
            }
        }
        out
    }

    /// `HordeUpdate::update` (HordeUpdate.cpp:243-377). The horde test runs for infantry on every wake and for other
    /// kinds only once `UpdateRate` frames have passed since the last test (:261-263). A test sets `m_inHorde`
    /// (`Count` members, or a true horde member within `RubOffRadius`, :276-291) and `m_trueHordeMember`, then
    /// `evaluateMoraleBonus` (:298-301). Returns the sleep the C++ returns.
    pub(crate) fn horde_update(&mut self, id: ObjectId, now: u32) -> u32 {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.horde.clone()) else { return UPDATE_SLEEP_FOREVER };
        let (is_infantry, last) = match self.objects.get(&id) {
            Some(o) => (o.template.is_kind_of(kind_of::INFANTRY), o.behavior.horde_last_refresh),
            None => return UPDATE_SLEEP_FOREVER,
        };
        if is_infantry || now > last.saturating_add(t.update_rate) {
            if let Some(o) = self.objects.get_mut(&id) {
                o.behavior.horde_last_refresh = now;
            }
            let members = self.horde_members(id, &t);
            let (in_horde, true_member) = if members.len() as i32 >= t.count - 1 {
                // "we really are in the thick part of the horde" (:270-273).
                (true, true)
            } else {
                // Not thick enough: a true horde member within `RubOffRadius` still rubs off (:276-291).
                let rub_off_sq = t.rub_off_radius * t.rub_off_radius;
                let me = self.objects.get(&id);
                let rubbed = members.iter().any(|m| match (me, self.objects.get(m)) {
                    (Some(me), Some(other)) => {
                        other.behavior.horde && other.behavior.horde_true_member && center_distance_2d_sq(me, other) <= rub_off_sq
                    }
                    _ => false,
                });
                (rubbed, false)
            };
            let has_ai = self.objects.get(&id).is_some_and(|o| o.template.ai_update.is_some());
            if let Some(o) = self.objects.get_mut(&id) {
                o.behavior.horde = in_horde;
                o.behavior.horde_true_member = true_member;
            }
            // `getAIUpdateInterface()` (:298-301): a unit without an AI has no morale bonus to evaluate.
            if has_ai {
                self.evaluate_morale_bonus(id, in_horde, t.allowed_nationalism);
            }
        }
        if is_infantry {
            t.update_rate
        } else {
            UPDATE_SLEEP_NONE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage::Relationship;
    use crate::sleepy_families::FamilyKind;
    use crate::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};
    use std::sync::Arc;

    fn horde_unit(name: &str, rate: u32, count: i32, radius: f32, infantry: bool) -> Arc<ObjectTemplate> {
        let mut t = ObjectTemplate::new(name);
        t.kind_of = (if infantry { kind_of::INFANTRY } else { kind_of::VEHICLE }) | kind_of::SELECTABLE;
        t.geometry.major_radius = 1.0;
        t.horde = Some(HordeTemplate {
            update_rate: rate,
            kind_of: 0,
            count,
            radius,
            rub_off_radius: 20.0,
            allies_only: true,
            exact_match: false,
            allowed_nationalism: true,
        });
        Arc::new(t)
    }

    fn logic() -> GameLogic {
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        g.add_player(0, 0);
        g
    }

    /// The heap key of `id`'s horde family slot (its wake frame shifted by the phase bits).
    fn horde_key(g: &GameLogic, id: ObjectId) -> Option<u32> {
        g.schedule
            .heap_order()
            .iter()
            .map(|&h| g.schedule.slot(h))
            .find(|s| s.object == id && s.family == Some(FamilyKind::Horde))
            .map(|s| s.next_call_and_phase)
    }

    /// `HordeUpdate::HordeUpdate` (HordeUpdate.cpp:169): the constructor draws `GameLogicRandomValue(1, UpdateRate)` at
    /// creation, one draw and no more, and the first wake is that many frames on. Revert check: without the
    /// `CtorDraw::Horde` the wake stays 0 and the generator is not advanced, so this fails.
    #[test]
    fn the_constructor_draws_the_first_wake_from_one_to_the_update_rate() {
        let mut g = logic();
        let t = horde_unit("Redguard", 30, 2, 15.0, true);
        let mut expect = g.rng.clone();
        let draw = expect.value(1, 30) as u32;
        let frame = g.frame();
        let id = g.create_object(&t, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
        assert_eq!(g.find_object(id).unwrap().behavior.horde_wake, frame + draw, "the constructor's wake");
        assert!(g.rng == expect, "exactly one constructor draw");
    }

    /// The module is a sleepy-heap entry asleep until its constructor wake (`registerObject`, GameLogic.cpp:4157-4200).
    /// Revert check: without the `Horde` registration there is no slot and this panics.
    #[test]
    fn the_horde_module_is_a_heap_entry_asleep_until_its_constructor_wake() {
        let mut g = logic();
        let t = horde_unit("Redguard", 30, 2, 15.0, true);
        let id = g.create_object(&t, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
        let wake = g.find_object(id).unwrap().behavior.horde_wake;
        assert_eq!(horde_key(&g, id).map(|k| k >> 2), Some(wake), "keyed at the constructor wake");
    }

    /// An infantry horde tests on every wake, and its wake is `UPDATE_SLEEP(UpdateRate)` (HordeUpdate.cpp:261-263, :377):
    /// the refreshes come `UpdateRate` frames apart, from the first wake. Revert check: a `NONE` return refreshes
    /// every frame and fails the spacing.
    #[test]
    fn an_infantry_horde_tests_again_every_update_rate_frames() {
        let mut g = logic();
        let t = horde_unit("Redguard", 30, 2, 15.0, true);
        let id = g.create_object(&t, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
        let wake = g.find_object(id).unwrap().behavior.horde_wake;
        let mut seen = Vec::new();
        let mut last = g.find_object(id).unwrap().behavior.horde_last_refresh;
        for _ in 0..100 {
            g.run_frames(1);
            let now = g.find_object(id).unwrap().behavior.horde_last_refresh;
            if now != last {
                seen.push(now);
                last = now;
            }
        }
        assert!(seen.len() >= 3, "several refreshes in 100 frames: {seen:?}");
        assert_eq!(seen[0], wake, "the first refresh is the constructor's wake");
        assert!(seen.windows(2).all(|w| w[1] - w[0] == 30), "every UpdateRate frames: {seen:?}");
    }

    /// A non-infantry horde (`UPDATE_SLEEP_NONE`) tests only once more than `UpdateRate` frames have passed since its
    /// last test (HordeUpdate.cpp:261-263). Revert check: without the gate the first refresh comes on the first wake.
    #[test]
    fn a_vehicle_horde_tests_only_after_its_update_rate() {
        let mut g = logic();
        let t = horde_unit("Tank", 30, 2, 15.0, false);
        let created = g.frame();
        let id = g.create_object(&t, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
        let mut seen = Vec::new();
        let mut last = g.find_object(id).unwrap().behavior.horde_last_refresh;
        for _ in 0..40 {
            g.run_frames(1);
            let now = g.find_object(id).unwrap().behavior.horde_last_refresh;
            if now != last {
                seen.push(now);
                last = now;
            }
        }
        assert_eq!(seen.first().copied(), Some(created + 31), "the first test is at last + UpdateRate + 1: {seen:?}");
    }

    /// `RubOffRadius` (HordeUpdate.cpp:276-291): a unit short of `Count` still joins the horde when a true horde
    /// member is within the rub-off radius. Layout (sphere radius 1, `Radius` 15): T has two members, X1 and X2, and
    /// the fringe F sees only T (F-X1 22 and F-X2 15.2 are out of `Radius`). Revert check: without the rub-off pass F
    /// stays out of the horde.
    #[test]
    fn the_rub_off_pass_gives_a_fringe_unit_the_horde_of_a_true_member() {
        let mut g = logic();
        let t = horde_unit("Redguard", 1, 3, 15.0, true);
        let centre = g.create_object(&t, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
        let x1 = g.create_object(&t, Coord3D::new(210.0, 200.0, 0.0), 0.0, 0);
        let x2 = g.create_object(&t, Coord3D::new(200.0, 210.0, 0.0), 0.0, 0);
        let fringe = g.create_object(&t, Coord3D::new(186.0, 200.0, 0.0), 0.0, 0);
        g.run_frames(5);
        let c = &g.find_object(centre).unwrap().behavior;
        assert!(c.horde && c.horde_true_member, "the centre has two members, a true member");
        assert!(g.find_object(x1).unwrap().behavior.horde, "X1 is a true member too");
        assert!(g.find_object(x2).unwrap().behavior.horde, "X2 is a true member too");
        let f = &g.find_object(fringe).unwrap().behavior;
        assert!(f.horde, "the fringe unit is rubbed off by the true member");
        assert!(!f.horde_true_member, "but it is not a true member itself");
    }

    /// `AlliesOnly` (HordeUpdate.cpp:103): an enemy next to a horde unit is not a member. Revert check: without the
    /// relationship test the enemy counts and the unit joins the horde.
    #[test]
    fn allies_only_leaves_an_enemy_out_of_the_horde() {
        let mut g = logic();
        g.add_player(1, 0);
        g.set_relationship_symmetric(0, 1, Relationship::Enemies);
        let t = horde_unit("Redguard", 1, 2, 100.0, true);
        let enemy = horde_unit("Gunner", 1, 2, 100.0, true);
        let id = g.create_object(&t, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
        g.create_object(&enemy, Coord3D::new(220.0, 200.0, 0.0), 0.0, 1);
        g.run_frames(3);
        assert!(!g.find_object(id).unwrap().behavior.horde, "an enemy is no member");
        let ally = g.create_object(&t, Coord3D::new(220.0, 220.0, 0.0), 0.0, 0);
        g.run_frames(3);
        assert!(g.find_object(id).unwrap().behavior.horde, "an ally makes the horde");
        assert!(g.find_object(ally).unwrap().behavior.horde);
    }

    /// `update` evaluates the morale bonus only through the object's AI (HordeUpdate.cpp:298-301, `getAIUpdateInterface`;
    /// the "must have an AI to benefit from horde" check at :201-208): the horde test still runs. Revert check: a
    /// bonus evaluated without an AI sets HORDE and fails this.
    #[test]
    fn a_horde_unit_without_an_ai_has_no_morale_bonus() {
        let mut g = logic();
        let t = horde_unit("Redguard", 1, 2, 100.0, true);
        let id = g.create_object(&t, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
        g.create_object(&t, Coord3D::new(220.0, 200.0, 0.0), 0.0, 0);
        g.run_frames(3);
        let o = g.find_object(id).unwrap();
        assert!(o.behavior.horde, "in the horde");
        assert_eq!(o.combat.weapon_bonus_conditions & (1 << crate::bonus::condition::HORDE), 0, "no AI, no morale bonus");
    }

    /// `ExactMatch` (HordeUpdate.cpp:90): only the same template counts. Revert check: without it the other
    /// template counts and the unit joins the horde with one neighbour.
    #[test]
    fn exact_match_counts_only_the_same_template() {
        let mut g = logic();
        let mut same_only = (*horde_unit("Redguard", 1, 2, 100.0, true)).clone();
        same_only.horde.as_mut().unwrap().exact_match = true;
        let same_only = Arc::new(same_only);
        let other = horde_unit("Gunner", 1, 2, 100.0, true);
        let id = g.create_object(&same_only, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
        g.create_object(&other, Coord3D::new(220.0, 200.0, 0.0), 0.0, 0);
        g.run_frames(3);
        assert!(!g.find_object(id).unwrap().behavior.horde, "a different template is no member");
        g.create_object(&same_only, Coord3D::new(220.0, 220.0, 0.0), 0.0, 0);
        g.run_frames(3);
        assert!(g.find_object(id).unwrap().behavior.horde, "the same template makes the horde");
    }
}
