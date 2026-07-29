//! Typed page documents: the autosurgeon-mapped shapes that live inside the
//! Automerge docs, plus conversions to the plain `mise_core` types the domain
//! math consumes.
//!
//! Structured pages are maps keyed by item/entry id, so concurrent edits
//! merge item-by-item and can never garble a page. Prose pages (recipes,
//! techniques) carry an Automerge text body plus a structured metadata map.
//!
//! Well-formedness invariants, normalized by the store's write paths:
//! string fields are trimmed, optional strings are `None` rather than empty,
//! and queue entries carry at least one dish.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;

use autosurgeon::{Hydrate, Reconcile, Text};
use jiff::civil::Date;
use mise_core::types::{
    LocationView, PantryItem, Portion, Presence, Slug, Tier,
};

use crate::error::{Result, StoreError};

/// Every doc carries this from day one.
pub const SCHEMA_VERSION: u32 = 1;

fn parse_date(s: &str, what: &str) -> Result<Date> {
    s.parse()
        .map_err(|e| StoreError::Corrupt(format!("{what}: bad date {s:?}: {e}")))
}

fn parse_slug(s: &str, what: &str) -> Result<Slug> {
    Slug::new(s).map_err(|e| StoreError::Corrupt(format!("{what}: {e}")))
}

// ---------------------------------------------------------------- state --

/// `state.md`: the active location plus the location registry.
#[derive(Clone, Debug, PartialEq, Reconcile, Hydrate)]
pub struct StateDoc {
    pub schema_version: u32,
    pub active_location: String,
    pub locations: BTreeMap<String, LocationMeta>,
}

#[derive(Clone, Debug, PartialEq, Reconcile, Hydrate)]
pub struct LocationMeta {
    /// "Usually cooking for 2 here."
    pub headcount: u32,
}

impl StateDoc {
    pub fn new(active: &str, headcount: u32) -> Self {
        StateDoc {
            schema_version: SCHEMA_VERSION,
            active_location: active.to_string(),
            locations: BTreeMap::from([(active.to_string(), LocationMeta { headcount })]),
        }
    }
}

// ---------------------------------------------------------------- queue --

#[derive(Clone, Debug, PartialEq, Reconcile, Hydrate)]
pub struct DishRefDoc {
    pub recipe: Option<String>,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Reconcile, Hydrate)]
pub struct QueueEntryDoc {
    pub dishes: Vec<DishRefDoc>,
    pub reason: Option<String>,
    /// ISO date the entry landed on the queue.
    pub added: String,
}

/// `queue.md` and `someday.md` share a shape: entries keyed by id.
#[derive(Clone, Debug, PartialEq, Reconcile, Hydrate)]
pub struct QueueDoc {
    pub schema_version: u32,
    pub entries: BTreeMap<String, QueueEntryDoc>,
}

impl QueueDoc {
    pub fn empty() -> Self {
        QueueDoc { schema_version: SCHEMA_VERSION, entries: BTreeMap::new() }
    }
}

// ------------------------------------------------------------- shopping --

#[derive(Clone, Debug, PartialEq, Reconcile, Hydrate)]
pub struct ShoppingItemDoc {
    pub text: String,
    /// Source tier id at the active location, if known.
    pub tier: Option<String>,
    pub done: bool,
}

#[derive(Clone, Debug, PartialEq, Reconcile, Hydrate)]
pub struct ShoppingDoc {
    pub schema_version: u32,
    pub items: BTreeMap<String, ShoppingItemDoc>,
}

impl ShoppingDoc {
    pub fn empty() -> Self {
        ShoppingDoc { schema_version: SCHEMA_VERSION, items: BTreeMap::new() }
    }
}

// ------------------------------------------------------ steering, facts --

/// `steering.md`: rotation goals and the skill agenda, as short keyed notes.
#[derive(Clone, Debug, PartialEq, Reconcile, Hydrate)]
pub struct SteeringDoc {
    pub schema_version: u32,
    pub entries: BTreeMap<String, String>,
}

impl SteeringDoc {
    pub fn empty() -> Self {
        SteeringDoc { schema_version: SCHEMA_VERSION, entries: BTreeMap::new() }
    }
}

/// `facts.md`: standing facts — many small facts, keyed by slug-ish key.
#[derive(Clone, Debug, PartialEq, Reconcile, Hydrate)]
pub struct FactsDoc {
    pub schema_version: u32,
    pub facts: BTreeMap<String, String>,
}

impl FactsDoc {
    pub fn empty() -> Self {
        FactsDoc { schema_version: SCHEMA_VERSION, facts: BTreeMap::new() }
    }
}

// --------------------------------------------------------------- pantry --

#[derive(Clone, Debug, PartialEq, Reconcile, Hydrate)]
pub struct PantryItemDoc {
    pub name: String,
    /// "have" | "low" | "out".
    pub presence: String,
    /// ISO date, rough purchase date for perishables.
    pub bought: Option<String>,
    /// Source tier id, as defined by this location's shops page.
    pub tier: Option<String>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Reconcile, Hydrate)]
pub struct PantryDoc {
    pub schema_version: u32,
    pub items: BTreeMap<String, PantryItemDoc>,
}

impl PantryDoc {
    pub fn empty() -> Self {
        PantryDoc { schema_version: SCHEMA_VERSION, items: BTreeMap::new() }
    }
}

impl PantryItemDoc {
    pub fn to_core(&self, slug: &str) -> Result<PantryItem> {
        let what = format!("pantry item {slug}");
        Ok(PantryItem {
            slug: parse_slug(slug, &what)?,
            name: self.name.clone(),
            presence: self
                .presence
                .parse::<Presence>()
                .map_err(|e| StoreError::Corrupt(format!("{what}: {e}")))?,
            bought: self.bought.as_deref().map(|d| parse_date(d, &what)).transpose()?,
            tier: self.tier.as_deref().map(|t| parse_slug(t, &what)).transpose()?,
            note: self.note.clone(),
        })
    }
}

// ------------------------------------------------------------ equipment --

/// `equipment.md`: what this kitchen has, slug → free-form note (often empty).
#[derive(Clone, Debug, PartialEq, Reconcile, Hydrate)]
pub struct EquipmentDoc {
    pub schema_version: u32,
    pub items: BTreeMap<String, String>,
}

impl EquipmentDoc {
    pub fn empty() -> Self {
        EquipmentDoc { schema_version: SCHEMA_VERSION, items: BTreeMap::new() }
    }
}

// ---------------------------------------------------------------- shops --

#[derive(Clone, Debug, PartialEq, Reconcile, Hydrate)]
pub struct TierDoc {
    pub id: String,
    pub name: String,
}

/// `shops.md`: the location's ordered source tiers, nearest first.
#[derive(Clone, Debug, PartialEq, Reconcile, Hydrate)]
pub struct ShopsDoc {
    pub schema_version: u32,
    pub tiers: Vec<TierDoc>,
}

impl ShopsDoc {
    pub fn new(tiers: &[(&str, &str)]) -> Self {
        ShopsDoc {
            schema_version: SCHEMA_VERSION,
            tiers: tiers
                .iter()
                .map(|(id, name)| TierDoc { id: id.to_string(), name: name.to_string() })
                .collect(),
        }
    }
}

// --------------------------------------------------------------- fridge --

#[derive(Clone, Debug, PartialEq, Reconcile, Hydrate)]
pub struct PortionDoc {
    pub dish: String,
    pub servings: u32,
    /// Cooked date (fridge) or frozen date (freezer), ISO.
    pub date: String,
}

impl PortionDoc {
    pub fn to_core(&self, id: &str) -> Result<Portion> {
        Ok(Portion {
            dish: self.dish.clone(),
            servings: self.servings,
            date: parse_date(&self.date, &format!("portion {id}"))?,
        })
    }
}

/// `fridge.md`: fridge state plus any number of named freezers.
#[derive(Clone, Debug, PartialEq, Reconcile, Hydrate)]
pub struct FridgeDoc {
    pub schema_version: u32,
    pub fridge: BTreeMap<String, PortionDoc>,
    pub freezers: BTreeMap<String, BTreeMap<String, PortionDoc>>,
}

impl FridgeDoc {
    pub fn empty() -> Self {
        FridgeDoc {
            schema_version: SCHEMA_VERSION,
            fridge: BTreeMap::new(),
            freezers: BTreeMap::new(),
        }
    }
}

// --------------------------------------------------------------- recipe --

#[derive(Clone, Debug, PartialEq, Reconcile, Hydrate)]
pub struct IngredientDoc {
    pub text: String,
    /// Explicit pantry-item slug link; never guessed by name matching.
    pub pantry: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Reconcile, Hydrate)]
pub struct LeadTimeDoc {
    pub minutes: u32,
    pub act_now_step: String,
}

/// `recipes/<slug>.md`: structured metadata plus a prose body.
#[derive(Clone, Debug, Reconcile, Hydrate)]
pub struct RecipeDoc {
    pub schema_version: u32,
    pub title: String,
    pub servings: u32,
    /// "weekday" | "project".
    pub effort: String,
    pub lead: Option<LeadTimeDoc>,
    /// Rotation axes: cuisine / protein / format by convention.
    pub tags: BTreeMap<String, String>,
    /// Required equipment slugs, in page order.
    pub equipment: Vec<String>,
    pub ingredients: Vec<IngredientDoc>,
    pub retired: bool,
    /// The method, written for the primary kitchen.
    pub body: Text,
}

impl PartialEq for RecipeDoc {
    fn eq(&self, other: &Self) -> bool {
        self.schema_version == other.schema_version
            && self.title == other.title
            && self.servings == other.servings
            && self.effort == other.effort
            && self.lead == other.lead
            && self.tags == other.tags
            && self.equipment == other.equipment
            && self.ingredients == other.ingredients
            && self.retired == other.retired
            && self.body.as_str() == other.body.as_str()
    }
}

impl RecipeDoc {
    pub fn to_core(&self, slug: &Slug) -> Result<mise_core::types::RecipeMeta> {
        let what = format!("recipe {slug}");
        let corrupt = |m: String| StoreError::Corrupt(format!("{what}: {m}"));
        Ok(mise_core::types::RecipeMeta {
            slug: slug.clone(),
            title: self.title.clone(),
            servings: NonZeroU32::new(self.servings)
                .ok_or_else(|| corrupt("zero servings".into()))?,
            effort: self.effort.parse().map_err(corrupt)?,
            lead_time: self
                .lead
                .as_ref()
                .map(|l| -> Result<mise_core::types::LeadTime> {
                    Ok(mise_core::types::LeadTime {
                        minutes: NonZeroU32::new(l.minutes)
                            .ok_or_else(|| corrupt("zero lead minutes".into()))?,
                        act_now_step: l.act_now_step.clone(),
                    })
                })
                .transpose()?,
            tags: self.tags.clone(),
            equipment: self
                .equipment
                .iter()
                .map(|e| parse_slug(e, &what))
                .collect::<Result<BTreeSet<_>>>()?,
            ingredients: self
                .ingredients
                .iter()
                .map(|i| -> Result<mise_core::types::IngredientLine> {
                    Ok(mise_core::types::IngredientLine {
                        text: i.text.clone(),
                        pantry: i.pantry.as_deref().map(|p| parse_slug(p, &what)).transpose()?,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
            retired: self.retired,
        })
    }
}

// ------------------------------------------------------------ technique --

#[derive(Clone, Debug, Reconcile, Hydrate)]
pub struct TechniqueDoc {
    pub schema_version: u32,
    pub title: String,
    pub tags: BTreeMap<String, String>,
    pub body: Text,
}

impl PartialEq for TechniqueDoc {
    fn eq(&self, other: &Self) -> bool {
        self.schema_version == other.schema_version
            && self.title == other.title
            && self.tags == other.tags
            && self.body.as_str() == other.body.as_str()
    }
}

// --------------------------------------------------------------- corpus --

/// Every doc belonging to one location.
#[derive(Clone, Debug, PartialEq)]
pub struct LocationDocs {
    pub pantry: PantryDoc,
    pub equipment: EquipmentDoc,
    pub shops: ShopsDoc,
    pub fridge: FridgeDoc,
}

impl LocationDocs {
    pub fn empty_with_tiers(tiers: &[(&str, &str)]) -> Self {
        LocationDocs {
            pantry: PantryDoc::empty(),
            equipment: EquipmentDoc::empty(),
            shops: ShopsDoc::new(tiers),
            fridge: FridgeDoc::empty(),
        }
    }

    /// Assemble the plain view the domain math consumes.
    pub fn to_view(&self, name: &str, meta: &LocationMeta) -> Result<LocationView> {
        let headcount = NonZeroU32::new(meta.headcount)
            .ok_or_else(|| StoreError::Corrupt(format!("location {name}: zero headcount")))?;
        let mut pantry = BTreeMap::new();
        for (slug, item) in &self.pantry.items {
            let item = item.to_core(slug)?;
            pantry.insert(item.slug.clone(), item);
        }
        let equipment = self
            .equipment
            .items
            .keys()
            .map(|s| parse_slug(s, "equipment"))
            .collect::<Result<BTreeSet<_>>>()?;
        let tiers = self
            .shops
            .tiers
            .iter()
            .map(|t| Ok(Tier { id: parse_slug(&t.id, "tier")?, name: t.name.clone() }))
            .collect::<Result<Vec<_>>>()?;
        let fridge = self
            .fridge
            .fridge
            .iter()
            .map(|(id, p)| p.to_core(id))
            .collect::<Result<Vec<_>>>()?;
        let freezer = self
            .fridge
            .freezers
            .values()
            .flat_map(|portions| portions.iter())
            .map(|(id, p)| p.to_core(id))
            .collect::<Result<Vec<_>>>()?;
        Ok(LocationView {
            name: name.to_string(),
            headcount,
            tiers,
            pantry,
            equipment,
            fridge,
            freezer,
        })
    }
}

/// The whole corpus, hydrated: input to the render layer, and the shape the
/// export completeness parser reconstructs.
#[derive(Clone, Debug, PartialEq)]
pub struct CorpusState {
    pub state: StateDoc,
    pub queue: QueueDoc,
    pub someday: QueueDoc,
    pub shopping: ShoppingDoc,
    pub steering: SteeringDoc,
    pub facts: FactsDoc,
    pub locations: BTreeMap<String, LocationDocs>,
    pub recipes: BTreeMap<String, RecipeDoc>,
    pub techniques: BTreeMap<String, TechniqueDoc>,
    pub log: Vec<mise_core::types::LogEntry>,
}
