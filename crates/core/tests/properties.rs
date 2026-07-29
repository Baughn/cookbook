//! Property tests for the domain invariants named in the testing charter:
//! readiness is monotone in pantry and equipment, coverage is monotone in
//! fridge servings, lead-time readiness is consistent under time shift, and
//! rotation recency agrees with a naive scan of the log.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;

use jiff::ToSpan;
use jiff::civil::{Date, DateTime};
use mise_core::coverage::coverage;
use mise_core::readiness::{self};
use mise_core::rotation::recency;
use mise_core::types::*;
use proptest::collection::vec;
use proptest::prelude::*;

fn slug(s: &str) -> Slug {
    Slug::new(s).unwrap()
}

// Small pools of slugs so recipes and pantries overlap often enough to be
// interesting.
fn ing_slug() -> impl Strategy<Value = Slug> {
    (0usize..10).prop_map(|i| Slug::new(format!("i{i}")).unwrap())
}

fn equip_slug() -> impl Strategy<Value = Slug> {
    (0usize..6).prop_map(|i| Slug::new(format!("e{i}")).unwrap())
}

fn tiers() -> Vec<Tier> {
    ["staples", "shop", "butcher", "town"]
        .into_iter()
        .map(|id| Tier { id: slug(id), name: id.to_string() })
        .collect()
}

fn arb_date() -> impl Strategy<Value = Date> {
    (2000i16..2100, 1i8..=12, 1i8..=28).prop_map(|(y, m, d)| Date::new(y, m, d).unwrap())
}

fn arb_datetime() -> impl Strategy<Value = DateTime> {
    (arb_date(), 0i8..24, 0i8..60).prop_map(|(d, h, m)| d.at(h, m, 0, 0))
}

fn arb_presence() -> impl Strategy<Value = Presence> {
    prop_oneof![Just(Presence::Have), Just(Presence::Low), Just(Presence::Out)]
}

fn arb_pantry_item() -> impl Strategy<Value = PantryItem> {
    (
        ing_slug(),
        arb_presence(),
        // Index 4 names a tier that is not in the location's tier list, to
        // exercise the unknown-source path.
        proptest::option::of(0usize..5),
        proptest::option::of(arb_date()),
    )
        .prop_map(|(s, presence, tier, bought)| PantryItem {
            name: s.as_str().to_string(),
            slug: s,
            presence,
            bought,
            tier: tier.map(|i| {
                ["staples", "shop", "butcher", "town", "elsewhere"][i]
                    .parse()
                    .unwrap()
            }),
            note: None,
        })
}

fn arb_location() -> impl Strategy<Value = LocationView> {
    (
        vec(arb_pantry_item(), 0..12),
        vec(equip_slug(), 0..6),
        1u32..=6,
    )
        .prop_map(|(items, equipment, headcount)| LocationView {
            name: "home".into(),
            headcount: NonZeroU32::new(headcount).unwrap(),
            tiers: tiers(),
            pantry: items.into_iter().map(|i| (i.slug.clone(), i)).collect(),
            equipment: equipment.into_iter().collect(),
            fridge: vec![],
            freezer: vec![],
        })
}

fn arb_lead() -> impl Strategy<Value = LeadTime> {
    (1u32..=20_160).prop_map(|m| LeadTime {
        minutes: NonZeroU32::new(m).unwrap(),
        act_now_step: "act now".into(),
    })
}

fn arb_recipe() -> impl Strategy<Value = RecipeMeta> {
    (
        vec((proptest::option::of(ing_slug()),), 0..8),
        vec(equip_slug(), 0..4),
        proptest::option::of(arb_lead()),
    )
        .prop_map(|(ingredients, equipment, lead_time)| RecipeMeta {
            slug: slug("dish"),
            title: "Dish".into(),
            servings: NonZeroU32::new(4).unwrap(),
            effort: EffortClass::Weekday,
            lead_time,
            tags: BTreeMap::new(),
            equipment: equipment.into_iter().collect(),
            ingredients: ingredients
                .into_iter()
                .enumerate()
                .map(|(i, (pantry,))| IngredientLine { text: format!("ingredient {i}"), pantry })
                .collect(),
            retired: false,
        })
}

/// Something a shop trip or a purchase can do to a location: more equipment,
/// or an item stocked (Have or Low — presence, not quantity).
#[derive(Clone, Debug)]
enum Improvement {
    Equip(Slug),
    Stock(Slug, Presence),
}

fn arb_improvement() -> impl Strategy<Value = Improvement> {
    prop_oneof![
        equip_slug().prop_map(Improvement::Equip),
        (ing_slug(), prop_oneof![Just(Presence::Have), Just(Presence::Low)])
            .prop_map(|(s, p)| Improvement::Stock(s, p)),
    ]
}

fn arb_portion() -> impl Strategy<Value = Portion> {
    (0u32..=12, arb_date()).prop_map(|(servings, date)| Portion {
        dish: "leftovers".into(),
        servings,
        date,
    })
}

fn arb_log_entry() -> impl Strategy<Value = LogEntry> {
    (
        arb_date(),
        vec((0usize..3, 0usize..3), 0..3),
    )
        .prop_map(|(date, tags)| LogEntry {
            date,
            kind: CookKind::Meal,
            recipe: None,
            title: "dish".into(),
            location: "home".into(),
            servings: 2,
            verdict: "fine".into(),
            tags: tags
                .into_iter()
                .map(|(a, v)| {
                    (
                        ["cuisine", "protein", "format"][a].to_string(),
                        ["a", "b", "c"][v].to_string(),
                    )
                })
                .collect(),
        })
}

proptest! {
    /// Adding pantry items or equipment never makes a dish less ready:
    /// every blocking set shrinks (or holds), the verdict never worsens, and
    /// unlinked-ingredient honesty is unaffected.
    #[test]
    fn readiness_is_monotone(
        recipe in arb_recipe(),
        loc in arb_location(),
        improvements in vec(arb_improvement(), 1..4),
    ) {
        let before = readiness::assess(&recipe, &loc);

        let mut better = loc.clone();
        for imp in improvements {
            match imp {
                Improvement::Equip(e) => {
                    better.equipment.insert(e);
                }
                Improvement::Stock(s, presence) => {
                    better.pantry.insert(
                        s.clone(),
                        PantryItem {
                            name: s.as_str().to_string(),
                            slug: s,
                            presence,
                            bought: None,
                            tier: None,
                            note: None,
                        },
                    );
                }
            }
        }
        let after = readiness::assess(&recipe, &better);

        prop_assert!(after.missing_equipment.is_subset(&before.missing_equipment));
        let needed_before: BTreeSet<&Slug> = before.shop.iter().map(|n| &n.item).collect();
        let needed_after: BTreeSet<&Slug> = after.shop.iter().map(|n| &n.item).collect();
        prop_assert!(needed_after.is_subset(&needed_before));
        prop_assert_eq!(&after.unlinked, &before.unlinked);
        prop_assert!(
            after.verdict(&better.tiers).rank() <= before.verdict(&loc.tiers).rank(),
            "verdict worsened: {:?} -> {:?}",
            before.verdict(&loc.tiers),
            after.verdict(&better.tiers),
        );
    }

    /// The defining lead-time identity: ready at `t` with lead `L` ⟺ the
    /// act-now step is due at `t − L`, exactly, in both directions.
    #[test]
    fn lead_time_is_consistent_under_time_shift(
        t in arb_datetime(),
        lead in arb_lead(),
    ) {
        let act = readiness::act_by(t, &lead);
        prop_assert_eq!(act, t.checked_sub(lead.duration()).unwrap());
        prop_assert_eq!(readiness::ready_at(act, &lead), t);
        prop_assert_eq!(
            readiness::ready_at(t, &lead),
            t.checked_add(lead.duration()).unwrap()
        );
    }

    /// Coverage counts dinners: floor(servings / headcount), runs out exactly
    /// that many days from today, is monotone in fridge servings, and the
    /// freezer only ever extends the horizon.
    #[test]
    fn coverage_is_monotone_and_exact(
        fridge in vec(arb_portion(), 0..8),
        freezer in vec(arb_portion(), 0..8),
        headcount in 1u32..=6,
        today in arb_date(),
        extra in arb_portion(),
    ) {
        let h = NonZeroU32::new(headcount).unwrap();
        let c = coverage(&fridge, &freezer, h, today);

        let servings: u64 = fridge.iter().map(|p| u64::from(p.servings)).sum();
        prop_assert_eq!(u64::from(c.dinners), servings / u64::from(headcount));
        prop_assert_eq!(c.runs_out, today.checked_add(i64::from(c.dinners).days()).unwrap());
        prop_assert!(c.runs_out_with_freezer >= c.runs_out);

        // Monotone: another portion never brings the runs-out date closer.
        let mut more = fridge.clone();
        more.push(extra);
        let c2 = coverage(&more, &freezer, h, today);
        prop_assert!(c2.dinners >= c.dinners);
        prop_assert!(c2.runs_out >= c.runs_out);

        // Defrosting (freezer → fridge) shifts dinners between buckets but
        // never changes the total horizon.
        if let Some(moved) = freezer.first() {
            let mut fridge2 = fridge.clone();
            fridge2.push(moved.clone());
            let freezer2 = freezer[1..].to_vec();
            let c3 = coverage(&fridge2, &freezer2, h, today);
            prop_assert_eq!(c3.runs_out_with_freezer, c.runs_out_with_freezer);
            prop_assert_eq!(
                c3.dinners + c3.freezer_dinners,
                c.dinners + c.freezer_dinners
            );
        }
    }

    /// Rotation recency agrees with a naive per-key scan of the log, and
    /// covers exactly the (axis, value) pairs cooked up to today.
    #[test]
    fn recency_agrees_with_naive_scan(
        log in vec(arb_log_entry(), 0..20),
        today in arb_date(),
        window in 1u16..60,
    ) {
        let r = recency(&log, today, window);

        for ((axis, value), rec) in &r {
            let dates: Vec<Date> = log
                .iter()
                .filter(|e| e.date <= today && e.tags.get(axis) == Some(value))
                .map(|e| e.date)
                .collect();
            prop_assert!(!dates.is_empty());
            prop_assert_eq!(rec.last_made, dates.iter().copied().max().unwrap());
            prop_assert!(rec.last_made <= today);
            let in_window = dates
                .iter()
                .filter(|d| (today - **d).get_days() < i32::from(window))
                .count();
            prop_assert_eq!(rec.in_window as usize, in_window);
        }

        for e in log.iter().filter(|e| e.date <= today) {
            for (a, v) in &e.tags {
                prop_assert!(r.contains_key(&(a.clone(), v.clone())));
            }
        }
    }
}
