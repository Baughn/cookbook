//! Can this dish be made right now, here — and if not, what exactly is in the
//! way? Readiness is computed against the active location: its pantry, its
//! equipment, its source tiers. Lead time makes a dish "ready tomorrow if you
//! start tonight"; the act-now step is surfaced, never silently absorbed.

use std::collections::BTreeSet;

use jiff::civil::DateTime;

use crate::types::{LeadTime, LocationView, Presence, RecipeMeta, Slug, Tier};

/// One missing ingredient and the tier that can supply it. `tier: None` means
/// the item's source is unknown (not in the pantry page, or no tier set).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShopNeed {
    pub item: Slug,
    pub tier: Option<Slug>,
}

/// The full readiness picture for one recipe at one location. The verdict is
/// derived; the parts are kept so the queue can say *why*.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Readiness {
    /// Required equipment the location doesn't have. Not shoppable.
    pub missing_equipment: BTreeSet<Slug>,
    /// Linked ingredients that are out (or absent from the pantry page).
    pub shop: Vec<ShopNeed>,
    /// Ingredient lines with no pantry link: honestly unknown, never guessed
    /// by name matching. Surfaced as "needs linking" alongside the verdict.
    pub unlinked: Vec<String>,
    pub lead_time: Option<LeadTime>,
}

/// The one-glance answer, ordered worst-first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Required equipment is missing here; no shop trip fixes that.
    MissingEquipment,
    /// Needs a shop trip; `tier` is the farthest tier required, `None` if any
    /// needed item has an unknown source.
    NeedsShopping { tier: Option<Slug> },
    /// Everything is on hand, but the act-now step gates it.
    AfterLead(LeadTime),
    Ready,
}

impl Verdict {
    /// Rank for "never gets worse" comparisons; lower is better.
    pub fn rank(&self) -> u8 {
        match self {
            Verdict::Ready => 0,
            Verdict::AfterLead(_) => 1,
            Verdict::NeedsShopping { .. } => 2,
            Verdict::MissingEquipment => 3,
        }
    }
}

pub fn assess(recipe: &RecipeMeta, location: &LocationView) -> Readiness {
    let missing_equipment = recipe
        .equipment
        .difference(&location.equipment)
        .cloned()
        .collect();

    // Keyed by pantry item: a recipe may legitimately name one item on
    // several lines (soy sauce in the marinade and again in the sauce), and
    // that is one thing to buy. The tier comes from the pantry entry, so
    // duplicates always agree on it.
    let mut shop = std::collections::BTreeMap::new();
    let mut unlinked = Vec::new();
    for line in &recipe.ingredients {
        match &line.pantry {
            None => unlinked.push(line.text.clone()),
            Some(slug) => match location.pantry.get(slug) {
                Some(item) if item.presence != Presence::Out => {}
                Some(item) => {
                    shop.insert(slug.clone(), item.tier.clone());
                }
                None => {
                    shop.insert(slug.clone(), None);
                }
            },
        }
    }
    let shop = shop.into_iter().map(|(item, tier)| ShopNeed { item, tier }).collect();

    Readiness {
        missing_equipment,
        shop,
        unlinked,
        lead_time: recipe.lead_time.clone(),
    }
}

impl Readiness {
    /// Derive the one-glance verdict. `tiers` is the location's ordered tier
    /// list; the shopping tier is the farthest one needed, and a need whose
    /// tier is unknown (or not in the list) makes the whole trip tier unknown.
    pub fn verdict(&self, tiers: &[Tier]) -> Verdict {
        if !self.missing_equipment.is_empty() {
            return Verdict::MissingEquipment;
        }
        if !self.shop.is_empty() {
            let mut farthest: Option<usize> = None;
            for need in &self.shop {
                let ordinal = need
                    .tier
                    .as_ref()
                    .and_then(|id| tiers.iter().position(|t| &t.id == id));
                match ordinal {
                    None => return Verdict::NeedsShopping { tier: None },
                    Some(i) => farthest = Some(farthest.map_or(i, |f| f.max(i))),
                }
            }
            let tier = farthest.map(|i| tiers[i].id.clone());
            return Verdict::NeedsShopping { tier };
        }
        match &self.lead_time {
            Some(lead) => Verdict::AfterLead(lead.clone()),
            None => Verdict::Ready,
        }
    }
}

/// If you act now, when could this be on the table?
pub fn ready_at(now: DateTime, lead: &LeadTime) -> DateTime {
    now.saturating_add(lead.duration())
}

/// To eat at `target`, when is the act-now step due? The defining identity —
/// ready at `t` with lead `L` ⟺ act-now step due at `t − L` — is a property
/// test in `tests/properties.rs`.
pub fn act_by(target: DateTime, lead: &LeadTime) -> DateTime {
    target.saturating_sub(lead.duration())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::num::NonZeroU32;

    use jiff::civil::Date;

    use super::*;
    use crate::types::{EffortClass, IngredientLine, PantryItem, RecipeStatus};

    fn slug(s: &str) -> Slug {
        Slug::new(s).unwrap()
    }

    fn item(s: &str, presence: Presence, tier: Option<&str>) -> (Slug, PantryItem) {
        (
            slug(s),
            PantryItem {
                slug: slug(s),
                name: s.replace('-', " "),
                presence,
                bought: None,
                tier: tier.map(slug),
                note: None,
            },
        )
    }

    fn home() -> LocationView {
        LocationView {
            name: "home".into(),
            headcount: NonZeroU32::new(2).unwrap(),
            tiers: vec![
                Tier { id: slug("staples"), name: "Staples".into() },
                Tier { id: slug("shop"), name: "Walkable shop".into() },
                Tier { id: slug("butcher"), name: "Butcher".into() },
                Tier { id: slug("town"), name: "Town".into() },
            ],
            pantry: BTreeMap::from([
                item("soy-sauce", Presence::Have, Some("staples")),
                item("miso", Presence::Out, Some("town")),
                item("chicken-thighs", Presence::Out, Some("butcher")),
                item("rice", Presence::Low, Some("staples")),
            ]),
            equipment: BTreeSet::from([slug("wok")]),
            fridge: vec![],
            freezer: vec![],
        }
    }

    fn recipe(ingredients: Vec<IngredientLine>, equipment: &[&str]) -> RecipeMeta {
        RecipeMeta {
            slug: slug("test-dish"),
            title: "Test dish".into(),
            servings: NonZeroU32::new(4).unwrap(),
            effort: EffortClass::Weekday,
            lead_time: None,
            tags: BTreeMap::new(),
            equipment: equipment.iter().map(|e| slug(e)).collect(),
            ingredients,
            status: RecipeStatus::Active,
        }
    }

    fn linked(text: &str, pantry: &str) -> IngredientLine {
        IngredientLine { text: text.into(), pantry: Some(slug(pantry)) }
    }

    #[test]
    fn ready_when_everything_is_on_hand() {
        let r = recipe(
            vec![linked("soy sauce", "soy-sauce"), linked("rice", "rice")],
            &["wok"],
        );
        let loc = home();
        let readiness = assess(&r, &loc);
        assert_eq!(readiness.verdict(&loc.tiers), Verdict::Ready);
        // Low counts as present: presence, not quantity.
    }

    #[test]
    fn missing_equipment_beats_shopping() {
        let r = recipe(vec![linked("miso", "miso")], &["stand-mixer"]);
        let loc = home();
        let readiness = assess(&r, &loc);
        assert_eq!(readiness.verdict(&loc.tiers), Verdict::MissingEquipment);
        // The shopping need is still visible for the UI:
        assert_eq!(readiness.shop.len(), 1);
    }

    #[test]
    fn shopping_tier_is_the_farthest_needed() {
        let r = recipe(
            vec![linked("chicken", "chicken-thighs"), linked("miso", "miso")],
            &[],
        );
        let loc = home();
        match assess(&r, &loc).verdict(&loc.tiers) {
            Verdict::NeedsShopping { tier: Some(t) } => assert_eq!(t.as_str(), "town"),
            v => panic!("expected town-tier shopping, got {v:?}"),
        }
    }

    #[test]
    fn one_pantry_item_on_two_lines_is_one_shop_need() {
        // Soy sauce in the marinade and again in the sauce is one item to
        // buy, not two entries in the shop verdict.
        let r = recipe(
            vec![
                linked("miso for the marinade", "miso"),
                linked("miso for the sauce", "miso"),
            ],
            &[],
        );
        let loc = home();
        let readiness = assess(&r, &loc);
        assert_eq!(
            readiness.shop,
            vec![ShopNeed { item: slug("miso"), tier: Some(slug("town")) }]
        );
    }

    #[test]
    fn unknown_source_makes_the_trip_tier_unknown() {
        let r = recipe(
            vec![linked("miso", "miso"), linked("wakame", "wakame")],
            &[],
        );
        let loc = home();
        assert_eq!(
            assess(&r, &loc).verdict(&loc.tiers),
            Verdict::NeedsShopping { tier: None }
        );
    }

    #[test]
    fn unlinked_lines_do_not_block_but_are_surfaced() {
        let r = recipe(
            vec![
                linked("soy sauce", "soy-sauce"),
                IngredientLine { text: "a splash of shaoxing wine".into(), pantry: None },
            ],
            &[],
        );
        let loc = home();
        let readiness = assess(&r, &loc);
        assert_eq!(readiness.verdict(&loc.tiers), Verdict::Ready);
        assert_eq!(readiness.unlinked, vec!["a splash of shaoxing wine".to_string()]);
    }

    #[test]
    fn lead_time_gates_an_otherwise_ready_dish() {
        let mut r = recipe(vec![linked("soy sauce", "soy-sauce")], &[]);
        let lead = LeadTime {
            minutes: NonZeroU32::new(12 * 60).unwrap(),
            act_now_step: "start the marinade".into(),
        };
        r.lead_time = Some(lead.clone());
        let loc = home();
        assert_eq!(assess(&r, &loc).verdict(&loc.tiers), Verdict::AfterLead(lead));
    }

    #[test]
    fn act_by_and_ready_at_are_inverses_at_a_known_point() {
        let lead = LeadTime {
            minutes: NonZeroU32::new(720).unwrap(),
            act_now_step: "marinate".into(),
        };
        let now = Date::constant(2026, 7, 29).at(18, 0, 0, 0);
        assert_eq!(ready_at(now, &lead), Date::constant(2026, 7, 30).at(6, 0, 0, 0));
        assert_eq!(act_by(ready_at(now, &lead), &lead), now);
    }
}
