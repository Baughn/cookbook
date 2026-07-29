//! Shared domain types. Everything here is plain data: no IO, no clock access,
//! no merge machinery. Dates are civil dates ("bought Tuesday"), matching the
//! presence-and-rough-date inventory model.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::num::NonZeroU32;
use std::str::FromStr;

use jiff::SignedDuration;
use jiff::civil::Date;
use serde::{Deserialize, Serialize};

/// A validated identifier: non-empty runs of lowercase ASCII alphanumerics
/// separated by single hyphens (`chicken-thighs`, `wok`, `mapo-tofu`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Slug(String);

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid slug {0:?}: want lowercase alphanumeric runs separated by single hyphens")]
pub struct SlugError(pub String);

impl Slug {
    pub fn new(s: impl Into<String>) -> Result<Self, SlugError> {
        let s = s.into();
        let ok = !s.is_empty()
            && !s.starts_with('-')
            && !s.ends_with('-')
            && !s.contains("--")
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if ok { Ok(Slug(s)) } else { Err(SlugError(s)) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Slug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Slug {
    type Err = SlugError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Slug::new(s)
    }
}

impl TryFrom<String> for Slug {
    type Error = SlugError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Slug::new(s)
    }
}

impl From<Slug> for String {
    fn from(s: Slug) -> String {
        s.0
    }
}

impl AsRef<str> for Slug {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Answers "what can I make *tonight*?" at a glance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EffortClass {
    /// ≤ 1h on a weekday evening.
    Weekday,
    /// A weekend project.
    Project,
}

impl fmt::Display for EffortClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            EffortClass::Weekday => "weekday",
            EffortClass::Project => "project",
        })
    }
}

impl FromStr for EffortClass {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "weekday" => Ok(EffortClass::Weekday),
            "project" => Ok(EffortClass::Project),
            other => Err(format!("unknown effort class {other:?} (weekday|project)")),
        }
    }
}

/// Explicit recipe metadata: a duration plus a named act-now step. Minutes
/// granularity keeps the type exact and the export deterministic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeadTime {
    pub minutes: NonZeroU32,
    /// The step the queue surfaces instead of silently calling the dish
    /// makeable: "start the marinade", "move the shoulder to the fridge".
    pub act_now_step: String,
}

impl LeadTime {
    pub fn duration(&self) -> SignedDuration {
        SignedDuration::from_mins(i64::from(self.minutes.get()))
    }
}

/// One ingredient line. The link to a pantry item is explicit — maintained by
/// the assistant from M3 on — and never guessed by name matching.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngredientLine {
    /// Human text: "400 g chicken thighs".
    pub text: String,
    /// Pantry-item slug this line draws on, if linked.
    pub pantry: Option<Slug>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecipeMeta {
    pub slug: Slug,
    pub title: String,
    /// Base servings produced; the base for scaling and coverage math.
    pub servings: NonZeroU32,
    pub effort: EffortClass,
    pub lead_time: Option<LeadTime>,
    /// Rotation axes as free-form tags: cuisine / protein / format by
    /// convention, but rotation math runs over whatever axes exist.
    pub tags: BTreeMap<String, String>,
    /// Equipment slugs the recipe needs (wok, stand-mixer, ...).
    pub equipment: BTreeSet<Slug>,
    pub ingredients: Vec<IngredientLine>,
    pub retired: bool,
}

/// Presence, never gram counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Presence {
    Have,
    Low,
    Out,
}

impl fmt::Display for Presence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Presence::Have => "have",
            Presence::Low => "low",
            Presence::Out => "out",
        })
    }
}

impl FromStr for Presence {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "have" => Ok(Presence::Have),
            "low" => Ok(Presence::Low),
            "out" => Ok(Presence::Out),
            other => Err(format!("unknown presence {other:?} (have|low|out)")),
        }
    }
}

/// A source tier as defined by a location. Tiers are ordered nearest-first
/// (home: staples, walkable shop, butcher, town); they are not a fixed enum.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tier {
    pub id: Slug,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PantryItem {
    pub slug: Slug,
    pub name: String,
    pub presence: Presence,
    /// Rough purchase date for perishables — never expiry bookkeeping.
    pub bought: Option<Date>,
    /// Source tier id (defined by the location) to restock from.
    pub tier: Option<Slug>,
    pub note: Option<String>,
}

impl PantryItem {
    /// Days since purchase, if a purchase date is known and not in the future.
    /// Freshness decay is commentary, not readiness input.
    pub fn age_days(&self, today: Date) -> Option<i32> {
        let bought = self.bought?;
        let days = (today - bought).get_days();
        (days >= 0).then_some(days)
    }
}

/// A cooked batch sitting in the fridge or a freezer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Portion {
    pub dish: String,
    pub servings: u32,
    /// Cooked date for fridge portions, frozen date for freezer portions.
    pub date: Date,
}

/// Everything readiness and coverage need to know about a *place*, assembled
/// by the caller from the location's pages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocationView {
    pub name: String,
    /// "Usually cooking for 2 here."
    pub headcount: NonZeroU32,
    /// Ordered nearest-first.
    pub tiers: Vec<Tier>,
    pub pantry: BTreeMap<Slug, PantryItem>,
    pub equipment: BTreeSet<Slug>,
    pub fridge: Vec<Portion>,
    /// Portions across all of the location's freezers.
    pub freezer: Vec<Portion>,
}

/// Not every cook is a meal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CookKind {
    Meal,
    Bake,
    /// Tare, stock, chilli oil, pickles — feeds the pantry, not the fridge.
    Staple,
}

impl fmt::Display for CookKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CookKind::Meal => "meal",
            CookKind::Bake => "bake",
            CookKind::Staple => "staple",
        })
    }
}

impl FromStr for CookKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "meal" => Ok(CookKind::Meal),
            "bake" => Ok(CookKind::Bake),
            "staple" => Ok(CookKind::Staple),
            other => Err(format!("unknown cook kind {other:?} (meal|bake|staple)")),
        }
    }
}

/// One append-only cook-log row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub date: Date,
    pub kind: CookKind,
    pub recipe: Option<Slug>,
    pub title: String,
    pub location: String,
    pub servings: u32,
    /// Distilled debrief verdict, one line.
    pub verdict: String,
    /// Rotation tags, usually inherited from the recipe.
    pub tags: BTreeMap<String, String>,
}

/// A dish on the queue: a linked recipe, or a stub that is still just an idea.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DishRef {
    pub recipe: Option<Slug>,
    pub title: String,
}

/// One queue entry — usually a single dish, occasionally a small menu that
/// shops, scales, and cooks together.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEntry {
    pub id: Slug,
    pub dishes: Vec<DishRef>,
    /// Why it's here: "rotating away from curry", "uses the wakame".
    pub reason: Option<String>,
    pub added: Date,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_validation() {
        for good in ["a", "wok", "mapo-tofu", "chicken-thighs-2"] {
            assert!(Slug::new(good).is_ok(), "{good}");
        }
        for bad in ["", "-a", "a-", "a--b", "Wok", "søtpotet", "a_b", "a b"] {
            assert!(Slug::new(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn age_days_ignores_future_purchases() {
        let item = PantryItem {
            slug: Slug::new("chicken-thighs").unwrap(),
            name: "chicken thighs".into(),
            presence: Presence::Have,
            bought: Some(Date::constant(2026, 7, 28)),
            tier: None,
            note: None,
        };
        assert_eq!(item.age_days(Date::constant(2026, 7, 30)), Some(2));
        assert_eq!(item.age_days(Date::constant(2026, 7, 27)), None);
    }
}
