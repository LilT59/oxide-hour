//! Lane 193 (wave 23): `Object::defect` for the capture-building ability, and `Object::getRelationship`
//! with its undetected-defector rule.
//!
//! The capture trigger calls `target->defect(player->getDefaultTeam(), 1)` (SpecialAbilityUpdate.cpp:1468).
//! `Object::defect` (Object.cpp:6239-6370) sets the target's undetected-defector flag for `detectionTime`
//! frames, starts its defection timer, and switches the team. While the flag is set, the target reads as an
//! ALLY to every other object (`Object::getRelationship`, Object.cpp:1595-1613): the capturing unit's own
//! preparation then aborts on its next frame (`continuePreparation`, SpecialAbilityUpdate.cpp:1204-1209),
//! which goes to `startPacking(false)` and `AI_BUSY` (`:391-395`). The port had neither the flag nor the
//! ALLIES reading, so the capturing units stayed `AI_IDLE` (AH7 s1 f3349).

use crate::ai_states::CommandSource;
use crate::damage::Relationship;
use crate::logic::GameLogic;
use crate::object::{status, ObjectId, PlayerIndex};

impl GameLogic {
    /// `Object::getRelationship(that)` (Object.cpp:1595-1613): the object's own undetected-defector flag makes it
    /// NEUTRAL to others (`:1599-1600`), and an undetected-defector `that` is ALLIES to this object (`:1603-1604`);
    /// otherwise the team relation, which the port reduces to the owners' relation (`Team::getRelationship`).
    /// A missing `that` is NEUTRAL (`:1611`).
    pub(crate) fn object_relationship(&self, a: ObjectId, b: ObjectId) -> Relationship {
        let (Some(me), Some(that)) = (self.objects.get(&a), self.objects.get(&b)) else {
            return Relationship::Neutral;
        };
        // `if (myTeam && that) { ... } return NEUTRAL;` (Object.cpp:1598-1611): an object with no team is neutral.
        if me.team.is_none() {
            return Relationship::Neutral;
        }
        if me.undetected_defector {
            Relationship::Neutral
        } else if that.undetected_defector {
            Relationship::Allies
        } else {
            // `myTeam->getRelationship(that->getTeam())` (Object.cpp:1609), reduced to the owners (Team.cpp:1469-1495
            // without the script's team and player overrides, which are not modelled).
            self.relationship(me.player, that.player)
        }
    }

    /// `target->getTeam() == getObject()->getTeam()` (SpecialAbilityUpdate.cpp:270, :1010, :1448): the two objects'
    /// team pointers are equal. Two null teams are equal in C++ too; a synthetic world must give its objects teams
    /// (`init_from_sides`) rather than rely on an owner fallback here. A missing object is on no team.
    pub(crate) fn same_object_team(&self, a: ObjectId, b: ObjectId) -> bool {
        match (self.objects.get(&a), self.objects.get(&b)) {
            (Some(x), Some(y)) => x.team == y.team,
            _ => false,
        }
    }

    /// `Object::defect(newTeam, detectionTime)` (Object.cpp:6239-6370), the parts the port models. A contained
    /// object, a player that already owns it, and a building under construction or sold do not defect
    /// (`:6241-6257`). Then the production queue is refunded to the old owner (`:6261-6268`), the undetected-defector
    /// flag is set for `detection_frames` (`:6281`), the defection timer starts (`:6283-6284`,
    /// `ObjectDefectionHelper::startDefectionTimer`), the team switches to the new player (`setTeam`, `:6287`), an AI
    /// goes idle (`:6292-6298`), and the riders are kicked out when the contain module says so (`:6315-6320`).
    /// Not modelled: the radar event and the defector sounds (client only), the partition cell maintenance
    /// (`:6293`, the port re-derives the cash cells every frame, logic.rs), the parking places (`:6322-6331`) and the
    /// mines the structure owns (`:6333-6346`). Those are open gaps (ROADMAP: the airfield case).
    pub(crate) fn object_defect(&mut self, id: ObjectId, new_player: PlayerIndex, detection_frames: u32) {
        let Some(o) = self.objects.get(&id) else { return };
        if o.contained_by.is_some() || o.player == new_player {
            return;
        }
        // `testStatus(UNDER_CONSTRUCTION) || testStatus(SOLD)` (:6254-6259). `UNDER_CONSTRUCTION` is derived from the
        // object's state, not stored in `status_bits`, so the test reads the derived mask.
        if o.status_mask(self.frame) & (status::UNDER_CONSTRUCTION | status::SOLD) != 0 {
            return;
        }
        let has_ai = o.ai_handle.is_some();
        // `cancelAndRefundAllProduction` (:6261-6268): the queue is refunded to the controller before the switch.
        self.refund_all_production(id);
        self.set_undetected_defector(id, detection_frames > 0);
        // `ObjectDefectionHelper::startDefectionTimer(detectionTime)` with the default `withDefectorFX` (TRUE).
        self.start_defection_timer(id, detection_frames, true);
        self.set_object_team(id, new_player);
        if has_ai {
            self.ai_idle(id, CommandSource::Ai);
        }
        // `if (ct && ct->isKickOutOnCapture()) ct->removeAllContained(TRUE)` (:6315-6320).
        if self.kick_out_on_capture(id) {
            for unit in self.contained_units(id).to_vec() {
                self.remove_from_contain(id, unit);
            }
        }
    }

    /// `ContainModuleInterface::isKickOutOnCapture` for the object's contain module (`:6315`). `OpenContain` is TRUE
    /// (OpenContain.h:168), and so are its subclasses that do not override it. `CaveContain` and `TunnelContain` are
    /// FALSE (CaveContain.h:105, TunnelContain.h:123). `OverlordContain` answers for its redirected contain, the first
    /// passenger's (OverlordContain.cpp:266-271), and FALSE when there is none. No contain module is FALSE.
    pub(crate) fn kick_out_on_capture(&self, id: ObjectId) -> bool {
        let Some(contain) = self.objects.get(&id).and_then(|o| o.template.contain.clone()) else { return false };
        match contain.kind {
            crate::contain::ContainKind::Cave | crate::contain::ContainKind::Tunnel => false,
            crate::contain::ContainKind::Overlord => {
                let first = self.contained_units(id).first().copied();
                let redirected = first.and_then(|f| self.objects.get(&f)).and_then(|f| f.template.contain.clone());
                redirected.is_some_and(|c| !matches!(c.kind, crate::contain::ContainKind::Cave | crate::contain::ContainKind::Tunnel | crate::contain::ContainKind::Overlord))
            }
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::damage::Relationship;
    use crate::logic::GameLogic;
    use crate::object::{ObjectId, PlayerIndex};
    use crate::template::ObjectTemplate;
    use crate::terrain::TerrainLogic;
    use crate::math::Coord3D;
    use std::sync::Arc;

    fn two_units(p0: PlayerIndex, p1: PlayerIndex) -> (GameLogic, ObjectId, ObjectId) {
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        let t = Arc::new(ObjectTemplate::new("Unit"));
        let a = g.create_object(&t, Coord3D::new(10.0, 10.0, 0.0), 0.0, p0);
        let b = g.create_object(&t, Coord3D::new(20.0, 10.0, 0.0), 0.0, p1);
        // Each owner's team instance (the engine gives every object one; a synthetic world must make it, F6).
        g.objects.get_mut(&a).unwrap().team = Some(format!("teamp{p0}"));
        g.objects.get_mut(&b).unwrap().team = Some(format!("teamp{p1}"));
        (g, a, b)
    }

    /// `Object::getRelationship` (Object.cpp:1595-1613): a defector reads as an ally to others, and is neutral to
    /// everyone itself while it is undetected; a plain pair takes the owners' relation.
    #[test]
    fn an_undetected_defector_is_an_ally_to_others_and_neutral_to_itself() {
        let (mut g, a, b) = two_units(0, 1);
        g.set_enemies(0, 1);
        assert_eq!(g.object_relationship(a, b), Relationship::Enemies, "plain pair: the owners' relation");
        g.set_undetected_defector(b, true);
        assert_eq!(g.object_relationship(a, b), Relationship::Allies, "the defector is an ally to others");
        assert_eq!(g.object_relationship(b, a), Relationship::Neutral, "the defector is neutral to others");
        assert_eq!(g.object_relationship(a, ObjectId::INVALID), Relationship::Neutral, "no object is neutral");
    }

    /// `Object::defect(newTeam, 0)` (the car-bomb crate, ConvertToCarBombCrateCollide.cpp:133): the switch with no
    /// undetected-defector time (`setUndetectedDefector(detectionTime > 0)`, Object.cpp:6281), so the object is
    /// not an ally to others afterwards. A second defect of the same object to its own player is a no-op (:6251).
    #[test]
    fn a_defect_with_no_detection_time_switches_the_owner_without_a_defector_flag() {
        let (mut g, a, b) = two_units(0, 1);
        g.set_enemies(0, 1);
        // F17: start from a set flag, so the cleared state is the rule's effect (`setUndetectedDefector(detectionTime > 0)`).
        g.set_undetected_defector(b, true);
        g.object_defect(b, 0, 0);
        let o = g.objects.get(&b).unwrap();
        assert_eq!(o.player, 0, "the owner switched");
        assert!(!o.undetected_defector, "no detection time: the flag is cleared, not left set");
        assert_eq!(g.object_relationship(a, b), Relationship::Allies, "same owner now");
    }

    /// `testStatus(UNDER_CONSTRUCTION)` (Object.cpp:6254-6259): a building under construction does not defect. The
    /// status is derived from the object's state (object.rs `status_mask`), so the guard reads the derived mask (F3).
    #[test]
    fn an_under_construction_building_does_not_defect() {
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        g.add_player(0, 0);
        g.add_player(1, 0);
        let t = Arc::new(ObjectTemplate::new("Bldg"));
        let b = g.create_object_under_construction(&t, Coord3D::new(20.0, 10.0, 0.0), 0.0, 1);
        g.object_defect(b, 0, 1);
        assert_eq!(g.objects.get(&b).unwrap().player, 1, "still the old owner");
    }

    /// `ActionManager`'s colleague test (`target->getTeam() == getObject()->getTeam()`, SpecialAbilityUpdate.cpp:270,
    /// :1010, :1448) compares the team pointers. Two objects in one team instance are colleagues whatever their owners,
    /// two instances of one owner are not, and two null teams are equal in C++ too (no owner fallback, F6).
    #[test]
    fn colleagues_are_compared_by_team_instance() {
        let (mut g, a, b) = two_units(0, 1);
        g.objects.get_mut(&a).unwrap().team = Some("T1".into());
        g.objects.get_mut(&b).unwrap().team = Some("T2".into());
        g.objects.get_mut(&a).unwrap().player = 1;
        assert!(!g.same_object_team(a, b), "distinct team instances are not colleagues, owners aside");
        g.objects.get_mut(&a).unwrap().player = 0;
        g.objects.get_mut(&b).unwrap().team = Some("T1".into());
        assert!(g.same_object_team(a, b), "one team instance: colleagues, whatever the owner");
        g.objects.get_mut(&b).unwrap().team = None;
        assert!(!g.same_object_team(a, b), "a team against no team is not the same team");
        g.objects.get_mut(&a).unwrap().team = None;
        assert!(g.same_object_team(a, b), "two null teams are the same pointer (C++ null == null)");
    }
}
