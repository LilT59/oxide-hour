//! Lane 193 (round 3): the owner's cash-bounty fraction (`CashBountyPower`, CashBountyPower.cpp:129-181), set by
//! its three C++ hooks. The port set it at creation for every owner, so an owner without the power's required
//! science (or before the building was finished) killed for a bounty the engine does not pay. AH7 s1 f3760:
//! player 2 got 12 money for a Ranger kill that C++ pays nothing for (`ZR_L193_KILL` bounty=0).

use crate::object::ObjectId;

impl crate::logic::GameLogic {
    /// `CashBountyPower::onObjectCreated` (CashBountyPower.cpp:134-143): the owner's fraction is raised only when the
    /// owner has the power's required science, `hasScience(getRequiredScience())`. Without a `RequiredScience` the
    /// power's science is `SCIENCE_INVALID` (SpecialPower.cpp:218), and `Player::hasScience` is a list lookup that
    /// never finds it (Player.cpp:2792-2795), so no science and no power name both pay nothing here.
    pub(crate) fn cash_bounty_on_object_created(&mut self, id: ObjectId) {
        let Some(o) = self.objects.get(&id) else { return };
        let (Some(bounty), player) = (o.template.cash_bounty, o.player) else { return };
        let science = o.template.cash_bounty_power.as_deref().and_then(|p| self.special_powers.get(p)).and_then(|t| t.required_science.clone());
        let Some(science) = science else { return };
        if self.player_has_science(player, &science) {
            self.set_player_cash_bounty(player, bounty);
        }
    }

    /// `CashBountyPower::onSpecialPowerCreation` (CashBountyPower.cpp:172-181): the building's `SpecialPowerCreate`
    /// (finished building) and a science grant (`Player::addScience`, Player.cpp:2590-2600) raise the fraction
    /// with no science test.
    pub(crate) fn cash_bounty_on_special_power_creation(&mut self, id: ObjectId) {
        let Some(o) = self.objects.get(&id) else { return };
        let (Some(bounty), player) = (o.template.cash_bounty, o.player) else { return };
        self.set_player_cash_bounty(player, bounty);
    }

    /// Whether `power` is the object's `CashBountyPower` (its `SpecialPowerTemplate`).
    pub(crate) fn is_cash_bounty_power(&self, id: ObjectId, power: &str) -> bool {
        self.objects.get(&id).is_some_and(|o| o.template.cash_bounty.is_some() && o.template.cash_bounty_power.as_deref() == Some(power))
    }
}

#[cfg(test)]
mod tests {
    use crate::logic::GameLogic;
    use crate::math::Coord3D;
    use crate::special_power::SimSpecialPowerTemplate;
    use crate::template::{kind_of, ObjectTemplate};
    use crate::terrain::TerrainLogic;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    /// A bounty building (25%) owned by player 0. Its `CashBountyPower` names the power `Bounty` when `power` is set,
    /// and that power needs `science` when it is set.
    fn bounty_building_with(power: Option<&str>, science: Option<&str>) -> (GameLogic, crate::object::ObjectId) {
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        g.add_player(0, 0);
        g.set_special_powers(BTreeMap::from([(
            "Bounty".to_string(),
            Arc::new(SimSpecialPowerTemplate {
                name: "Bounty".into(),
                kind: "SPECIAL_CASH_BOUNTY".into(),
                reload_time: 0,
                required_science: science.map(str::to_string),
                shared_synced_timer: false,
                public_timer: false,
                radius_cursor_radius: 0.0,
            }),
        )]));
        let mut t = ObjectTemplate::new("Bank");
        t.kind_of = kind_of::STRUCTURE;
        t.cash_bounty = Some(0.25);
        t.cash_bounty_power = power.map(str::to_string);
        t.special_power_create = true;
        let id = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        (g, id)
    }

    fn bounty_building(science: Option<&str>) -> (GameLogic, crate::object::ObjectId) {
        bounty_building_with(Some("Bounty"), science)
    }

    /// The owner's cash-bounty fraction (`Player::getCashBounty`).
    fn fraction(g: &GameLogic) -> f32 {
        g.players.get(&0).map_or(-1.0, |p| p.cash_bounty)
    }

    /// `hasScience(SCIENCE_INVALID)` is FALSE (Player.cpp:2792-2795), and a power with no `RequiredScience` has
    /// `SCIENCE_INVALID` (SpecialPower.cpp:218). So `onObjectCreated` never pays for it (CashBountyPower.cpp:134-143).
    #[test]
    fn a_bounty_power_without_a_required_science_does_not_pay_at_creation() {
        let (g, _) = bounty_building(None);
        assert_eq!(fraction(&g), 0.0, "SCIENCE_INVALID is never held, so no fraction at creation");
    }

    /// A template with no `CashBountyPower` name has no science to test, so it is the same case: nothing at
    /// creation, and the finished building still sets the fraction (the build-complete hook has no science test).
    #[test]
    fn a_bounty_without_its_power_name_pays_only_at_build_complete() {
        let (mut g, id) = bounty_building_with(None, None);
        assert_eq!(fraction(&g), 0.0, "no power, no science, no fraction at creation");
        g.create_modules_on_build_complete(id);
        assert_eq!(fraction(&g), 0.25, "the finished building sets the fraction");
    }

    /// `CashBountyPower::onObjectCreated` (CashBountyPower.cpp:129-137): an owner without the power's required
    /// science gets no fraction at creation. The port paid it for every owner (AH7 s1 f3760). Revert: the science
    /// test removed (the fraction set at creation) fails this.
    #[test]
    fn the_bounty_is_not_paid_at_creation_without_the_required_science() {
        let (g, _) = bounty_building(Some("SCIENCE_BOUNTY"));
        assert_eq!(g.players.get(&0).map_or(-1.0, |p| p.cash_bounty), 0.0, "no science, no bounty yet");
    }

    /// `CashBountyPower::onSpecialPowerCreation` (CashBountyPower.cpp:172-181), run by `SpecialPowerCreate` when the
    /// building is finished: the fraction is set with no science test.
    #[test]
    fn the_bounty_is_paid_when_the_building_completes() {
        let (mut g, id) = bounty_building(Some("SCIENCE_BOUNTY"));
        g.create_modules_on_build_complete(id);
        assert_eq!(g.players.get(&0).map_or(-1.0, |p| p.cash_bounty), 0.25, "the finished building sets the fraction");
    }
}
