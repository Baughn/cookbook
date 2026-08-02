//! The tool set: the same operations the CLI and HTTP surface expose, as
//! deterministic functions over the store. No privileged side door — every
//! mutation goes through the ordinary typed-page path and records the
//! conversation's provenance. Nothing here exports; the caller exports once
//! per exchange.
//!
//! Error policy: anything the model can fix by trying again differently
//! (bad input, unknown slug, duplicate) comes back as an `is_error` tool
//! result; real store failures abort the exchange.

use std::collections::BTreeMap;

use jiff::civil::Date;
use serde::Deserialize;
use serde_json::{Value, json};

use mise_core::types::{CookKind, EffortClass, LogEntry, Presence, Slug};
use mise_store::pages::{
    DishRefDoc, EquipmentDoc, FactsDoc, FridgeDoc, IngredientDoc, LeadTimeDoc, PantryDoc,
    PantryItemDoc, PortionDoc, QueueDoc, QueueEntryDoc, RecipeDoc, ShoppingDoc, ShoppingItemDoc,
    StateDoc, SteeringDoc,
};
use mise_store::{DocId, Store, StoreError};

use crate::error::Result;
use crate::seam::ToolDef;
use crate::turn::{ToolCall, ToolOutcome};
use crate::views;

/// Ambient facts for one exchange: the clock (always a parameter, never
/// read here) and the provenance string edits are recorded under.
pub struct ToolCtx {
    pub now: jiff::Zoned,
    pub provenance: String,
}

impl ToolCtx {
    fn today(&self) -> Date {
        self.now.date()
    }

    /// Commit timestamp for this exchange's edits.
    fn at(&self) -> jiff::Timestamp {
        self.now.timestamp()
    }

    /// Change messages are immutable and replicate to every device, and
    /// `action` embeds model words that may quote a fetched page. One rule
    /// at the funnel, whatever the source: a single bounded line — control
    /// characters become spaces, so nothing can forge a second
    /// provenance-looking entry in the history view.
    fn msg(&self, action: &str) -> String {
        const MAX_CHARS: usize = 200;
        let line = format!("{}: {action}", self.provenance);
        let mut clean: String = line
            .split(|c: char| c.is_control() || c.is_whitespace())
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if clean.chars().count() > MAX_CHARS {
            clean = clean.chars().take(MAX_CHARS - 1).collect();
            clean.push('…');
        }
        clean
    }
}

/// Execute one tool call. Model-recoverable problems come back as an
/// `is_error` outcome; store failures propagate.
pub fn execute(store: &mut Store, ctx: &ToolCtx, call: &ToolCall) -> Result<ToolOutcome> {
    match run(store, ctx, &call.name, &call.input) {
        Ok(content) => Ok(ToolOutcome {
            tool_use_id: call.id.clone(),
            content,
            is_error: false,
        }),
        Err(Fail::User(content)) => Ok(ToolOutcome {
            tool_use_id: call.id.clone(),
            content,
            is_error: true,
        }),
        Err(Fail::Store(e)) => Err(crate::error::AssistantError::Store(e)),
    }
}

enum Fail {
    /// The model's problem: reported as an error tool result.
    User(String),
    /// Ours: aborts the exchange.
    Store(Box<StoreError>),
}

impl From<StoreError> for Fail {
    fn from(e: StoreError) -> Fail {
        match e {
            StoreError::NotFound(_)
            | StoreError::Exists(_)
            | StoreError::Invalid(_)
            | StoreError::BadDocId(_) => Fail::User(e.to_string()),
            other => Fail::Store(Box::new(other)),
        }
    }
}

type ToolResult = std::result::Result<String, Fail>;

fn user(msg: impl Into<String>) -> Fail {
    Fail::User(msg.into())
}

/// Every servings count crosses this on its way in. The bound is hygiene,
/// not arithmetic safety — coverage saturates on its own — but an absurd
/// number persisted once syncs to every replica forever.
fn bounded_servings(n: u32) -> std::result::Result<u32, Fail> {
    match n {
        0 => Err(user("servings must be at least 1")),
        1..=999 => Ok(n),
        _ => Err(user("servings above 999 is a typo, not a batch")),
    }
}

fn parse<T: serde::de::DeserializeOwned>(input: &Value) -> std::result::Result<T, Fail> {
    serde_json::from_value(input.clone()).map_err(|e| user(format!("bad tool input: {e}")))
}

fn slug(s: &str) -> std::result::Result<Slug, Fail> {
    Slug::new(s.trim()).map_err(|e| user(e.to_string()))
}

fn must_trim(s: &str, what: &str) -> std::result::Result<String, Fail> {
    let t = s.trim();
    if t.is_empty() {
        return Err(user(format!("{what} must not be empty")));
    }
    Ok(t.to_string())
}

fn opt_trim(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn parse_date(s: &str, today: Date) -> std::result::Result<Date, Fail> {
    if s.trim() == "today" {
        return Ok(today);
    }
    s.trim()
        .parse()
        .map_err(|_| user(format!("bad date {s:?} (want YYYY-MM-DD or \"today\")")))
}

fn clean_tags(
    tags: BTreeMap<String, String>,
) -> std::result::Result<BTreeMap<String, String>, Fail> {
    tags.into_iter()
        .map(|(k, v)| Ok((must_trim(&k, "tag axis")?, must_trim(&v, "tag value")?)))
        .collect()
}

fn resolve_location(
    store: &Store,
    requested: &Option<String>,
) -> std::result::Result<Slug, Fail> {
    let state: StateDoc = store.get(&DocId::State)?;
    match requested {
        Some(l) => {
            let l = slug(l)?;
            if !state.locations.contains_key(l.as_str()) {
                return Err(user(format!("no location {l}")));
            }
            Ok(l)
        }
        None => slug(&state.active_location),
    }
}

/// A tier must exist on the location's shops page. Readiness treats an
/// unknown tier exactly like a missing one, so a typo here would silently
/// erase the tier for every dish that needs the item — the error policy
/// says unknown slugs are the model's problem, loudly.
fn resolve_tier(
    store: &Store,
    loc: &Slug,
    requested: Option<&str>,
) -> std::result::Result<Option<Slug>, Fail> {
    let Some(t) = requested else { return Ok(None) };
    let tier = slug(t)?;
    let shops: mise_store::pages::ShopsDoc = store.get(&DocId::Shops(loc.clone()))?;
    if shops.tiers.iter().any(|t| t.id == tier.as_str()) {
        return Ok(Some(tier));
    }
    let known: Vec<&str> = shops.tiers.iter().map(|t| t.id.as_str()).collect();
    Err(user(format!("no tier {tier} at {loc}; tiers are: {}", known.join(", "))))
}

// -------------------------------------------------------------- schemas --

fn obj(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn s(description: &str) -> Value {
    json!({"type": "string", "description": description})
}

fn b(description: &str) -> Value {
    json!({"type": "boolean", "description": description})
}

fn n(description: &str) -> Value {
    json!({"type": "integer", "minimum": 0, "description": description})
}

fn tags_schema() -> Value {
    json!({
        "type": "object",
        "description": "Rotation tags, axis → value (cuisine=sichuan, protein=pork, format=braise).",
        "additionalProperties": {"type": "string"},
    })
}

fn location_schema() -> Value {
    s("Location slug; defaults to the active location.")
}

/// Every tool, in the order they are offered to the model.
pub fn tool_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "queue_status",
            description: "The queue with readiness annotations (ready now / start now for lead \
                          time / shop, by source tier / missing equipment), fridge coverage in \
                          dinners, and the someday shelf — for the active location.",
            input_schema: obj(json!({}), &[]),
        },
        ToolDef {
            name: "list_pages",
            description: "Every page in the corpus, by path. Recipes are annotated with title, \
                          rotation tags, and effort class.",
            input_schema: obj(json!({}), &[]),
        },
        ToolDef {
            name: "read_page",
            description: "Read one page by path (as shown by list_pages): recipes/mapo-tofu, \
                          locations/home/pantry, steering, log/2026-07, threads/planning, …",
            input_schema: obj(json!({"path": s("Page path; .md suffix optional.")}), &["path"]),
        },
        ToolDef {
            name: "search",
            description: "Case-insensitive substring search across every page. Returns \
                          path:line matches.",
            input_schema: obj(json!({"query": s("Substring to look for.")}), &["query"]),
        },
        ToolDef {
            name: "queue_add",
            description: "Put a dish (or a recipe-less idea) on the queue, or the someday \
                          shelf. Upserts by id: an existing entry keeps its age and its \
                          dishes, and only the reason is updated.",
            input_schema: obj(
                json!({
                    "title": s("Dish title as it should appear."),
                    "recipe": s("Recipe slug this dish cooks from, if any. Must exist."),
                    "reason": s("Why it's here: \"rotating away from curry\", \"uses the wakame\"."),
                    "id": s("Entry id; defaults to a slug of the title."),
                    "someday": b("Someday shelf instead of the active queue."),
                }),
                &["title"],
            ),
        },
        ToolDef {
            name: "queue_remove",
            description: "Remove a queue (or someday) entry by id.",
            input_schema: obj(
                json!({
                    "id": s("Entry id."),
                    "someday": b("Remove from the someday shelf instead."),
                }),
                &["id"],
            ),
        },
        ToolDef {
            name: "recipe_add",
            description: "Create a recipe page.",
            input_schema: obj(
                json!({
                    "slug": s("Recipe slug (lowercase, hyphenated)."),
                    "title": s("Title."),
                    "servings": n("Servings the method yields. Default 2."),
                    "effort": s("weekday or project. Default weekday."),
                    "tags": tags_schema(),
                    "equipment": {"type": "array", "items": s("Required equipment slug.")},
                    "lead_minutes": n("Lead time in minutes (with lead_step)."),
                    "lead_step": s("The act-now step: \"start the marinade\"."),
                    "ingredients": ingredients_schema(),
                    "body": s("Method body, markdown."),
                    "status": s("draft or active. Default active. Use draft for a recipe \
                                 nobody asked to cook yet — a URL worth keeping, an idea to \
                                 flesh out; the first logged cook promotes it."),
                    "source": s("Where it came from: the URL you drafted it from, if any."),
                }),
                &["slug", "title"],
            ),
        },
        ToolDef {
            name: "recipe_edit",
            description: "Edit a recipe. Only the fields you pass change; tags, equipment, and \
                          ingredients replace their lists wholesale, so pass the full new list \
                          when editing them.",
            input_schema: obj(
                json!({
                    "slug": s("Recipe slug."),
                    "title": s("New title."),
                    "servings": n("New servings."),
                    "effort": s("weekday or project."),
                    "tags": tags_schema(),
                    "equipment": {"type": "array", "items": s("Required equipment slug.")},
                    "lead_minutes": n("Lead time in minutes (with lead_step)."),
                    "lead_step": s("The act-now step."),
                    "clear_lead": b("Remove the lead time entirely."),
                    "ingredients": ingredients_schema(),
                    "body": s("Replacement method body, markdown."),
                    "status": s("draft, active, or retired."),
                    "source": s("Set (or replace) the URL the recipe came from."),
                }),
                &["slug"],
            ),
        },
        ToolDef {
            name: crate::fetch::FETCH_URL,
            description: "Fetch one web page the user pointed you at and get its readable \
                          content as markdown — recipe pages come back as clean structured \
                          data when the site provides it. Only for URLs the user explicitly \
                          gave you; never go browsing on your own.",
            input_schema: obj(json!({ "url": s("The URL, as the user gave it.") }), &["url"]),
        },
        ToolDef {
            name: crate::recon::PROPOSE_PANTRY_DIFF,
            description: "Propose pantry updates read off a photo, shown to the user as \
                          tappable lines — nothing changes until they tap. Use this instead \
                          of pantry_set whenever a photo is your evidence: photos get \
                          misread, and the user's corrections are ground truth. One line \
                          per item, each with the visible evidence as its reason. When the \
                          user corrects a proposal in words, answer with a fresh corrected \
                          proposal — never point them back at earlier lines.",
            input_schema: obj(
                json!({
                    "location": location_schema(),
                    "lines": {
                        "type": "array",
                        "description": "One proposed pantry-set per differing item.",
                        "items": obj(
                            json!({
                                "item": s("Pantry item slug."),
                                "presence": s("have, low, or out."),
                                "name": s("Display name, for items not yet on the page."),
                                "reason": s("What in the photo says so: \"open jar, half left\"."),
                            }),
                            &["item", "presence", "reason"],
                        ),
                    },
                }),
                &["lines"],
            ),
        },
        ToolDef {
            name: "pantry_set",
            description: "Create or update a pantry item. Only the fields you pass change. \
                          Presence is have/low/out — set out rather than removing when \
                          something runs dry.",
            input_schema: obj(
                json!({
                    "item": s("Pantry item slug."),
                    "name": s("Display name; defaults to the slug with spaces."),
                    "presence": s("have, low, or out."),
                    "tier": s("Source tier slug (see the location's shops page)."),
                    "bought": s("Rough purchase date, YYYY-MM-DD or \"today\"."),
                    "note": s("Free note; empty string clears it."),
                    "location": location_schema(),
                }),
                &["item"],
            ),
        },
        ToolDef {
            name: "pantry_remove",
            description: "Delete a pantry item entirely (usually presence=out is what you want).",
            input_schema: obj(
                json!({"item": s("Pantry item slug."), "location": location_schema()}),
                &["item"],
            ),
        },
        ToolDef {
            name: "equipment_set",
            description: "Record a piece of kitchen equipment at a location, optionally with a \
                          note. Only the fields you pass change; an empty note clears it.",
            input_schema: obj(
                json!({
                    "item": s("Equipment slug: wok, stand-mixer, …"),
                    "note": s("Free note."),
                    "location": location_schema(),
                }),
                &["item"],
            ),
        },
        ToolDef {
            name: "equipment_remove",
            description: "Remove a piece of equipment from a location.",
            input_schema: obj(
                json!({"item": s("Equipment slug."), "location": location_schema()}),
                &["item"],
            ),
        },
        ToolDef {
            name: "fridge_add",
            description: "Add a cooked batch to the fridge, or a named freezer.",
            input_schema: obj(
                json!({
                    "dish": s("What it is."),
                    "servings": n("Servings in the batch."),
                    "date": s("Cooked/frozen date, YYYY-MM-DD or \"today\" (default)."),
                    "freezer": s("Freezer name; omit for the fridge."),
                    "location": location_schema(),
                }),
                &["dish", "servings"],
            ),
        },
        ToolDef {
            name: "fridge_remove",
            description: "Remove a portion by id (eaten through, defrosted, gone).",
            input_schema: obj(
                json!({
                    "id": s("Portion id from the fridge page."),
                    "freezer": s("Freezer name if it's in one."),
                    "location": location_schema(),
                }),
                &["id"],
            ),
        },
        ToolDef {
            name: "log_append",
            description: "Append a cook to the log. With a recipe, servings and rotation tags \
                          inherit from it.",
            input_schema: obj(
                json!({
                    "title": s("What was cooked."),
                    "recipe": s("Recipe slug, if it cooked from one."),
                    "kind": s("meal, bake, or staple. Default meal."),
                    "servings": n("Servings produced; defaults to the recipe's."),
                    "verdict": s("One-line verdict. Default \"fine\"."),
                    "date": s("YYYY-MM-DD or \"today\" (default)."),
                    "tags": tags_schema(),
                    "location": location_schema(),
                }),
                &["title"],
            ),
        },
        ToolDef {
            name: "shopping_add",
            description: "Put an item on the shopping list.",
            input_schema: obj(
                json!({
                    "text": s("What to buy."),
                    "tier": s("Source tier slug (walkable shop, butcher, town…)."),
                    "id": s("Item id; generated if omitted."),
                }),
                &["text"],
            ),
        },
        ToolDef {
            name: "shopping_update",
            description: "Check off or remove a shopping-list item.",
            input_schema: obj(
                json!({
                    "id": s("Item id from the shopping page."),
                    "done": b("Mark bought (true) or un-bought (false)."),
                    "remove": b("Delete the row entirely."),
                }),
                &["id"],
            ),
        },
        ToolDef {
            name: "steering_set",
            description: "Set or clear an entry on the steering page — rotation goals and the \
                          skill agenda live here, visible and editable.",
            input_schema: obj(
                json!({
                    "key": s("Entry key."),
                    "note": s("The steering note; omit to remove the entry."),
                }),
                &["key"],
            ),
        },
        ToolDef {
            name: "facts_set",
            description: "Set or clear an entry on the facts page — durable knowledge about \
                          the people and household that no recipe owns.",
            input_schema: obj(
                json!({
                    "key": s("Entry key."),
                    "fact": s("The fact; omit to remove the entry."),
                }),
                &["key"],
            ),
        },
    ]
}

fn ingredients_schema() -> Value {
    json!({
        "type": "array",
        "items": obj(
            json!({
                "text": s("The ingredient line as written."),
                "pantry": s("Pantry item slug this line draws on, when known. Never guess."),
            }),
            &["text"],
        ),
        "description": "Ingredient lines, in order.",
    })
}

// ------------------------------------------------------------- dispatch --

fn run(store: &mut Store, ctx: &ToolCtx, name: &str, input: &Value) -> ToolResult {
    match name {
        "queue_status" => Ok(views::queue_status(store, ctx.now.datetime())?),
        "list_pages" => list_pages(store),
        "read_page" => read_page(store, input),
        "search" => search(store, input),
        "queue_add" => queue_add(store, ctx, input),
        "queue_remove" => queue_remove(store, ctx, input),
        "recipe_add" => recipe_add(store, ctx, input),
        "recipe_edit" => recipe_edit(store, ctx, input),
        "pantry_set" => pantry_set(store, ctx, input),
        "pantry_remove" => pantry_remove(store, ctx, input),
        "equipment_set" => equipment_set(store, ctx, input),
        "equipment_remove" => equipment_remove(store, ctx, input),
        "fridge_add" => fridge_add(store, ctx, input),
        "fridge_remove" => fridge_remove(store, ctx, input),
        "log_append" => log_append(store, ctx, input),
        "shopping_add" => shopping_add(store, ctx, input),
        "shopping_update" => shopping_update(store, ctx, input),
        "steering_set" => steering_set(store, ctx, input),
        "facts_set" => facts_set(store, ctx, input),
        other => Err(user(format!("no such tool: {other}"))),
    }
}

// ---------------------------------------------------------------- reads --

fn list_pages(store: &Store) -> ToolResult {
    let corpus = store.corpus().map_err(Fail::from)?;
    let files = mise_store::render::render(&corpus);
    let mut out = String::new();
    for path in files.keys() {
        let annotation = path
            .strip_prefix("recipes/")
            .and_then(|p| p.strip_suffix(".md"))
            .and_then(|slug| corpus.recipes.get(slug))
            .map(|r| {
                let tags = r
                    .tags
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(";");
                // Status carries planning semantics (drafts and retired
                // recipes stay out of rotation), so the listing is the
                // model's one cheap place to see it. Active is the normal
                // case and stays unannotated.
                let status = match r.status {
                    mise_core::types::RecipeStatus::Active => String::new(),
                    s => format!(", {s}"),
                };
                format!(" — {} [{}] ({}{status})", r.title, tags, r.effort)
            })
            .unwrap_or_default();
        out.push_str(&format!("{path}{annotation}\n"));
    }
    Ok(out)
}

fn read_page(store: &Store, input: &Value) -> ToolResult {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct In {
        path: String,
    }
    let In { path } = parse(input)?;
    let corpus = store.corpus().map_err(Fail::from)?;
    let files = mise_store::render::render(&corpus);
    let key = path.trim().trim_matches('/');
    let key = key.strip_suffix(".md").unwrap_or(key);
    files
        .get(&format!("{key}.md"))
        .cloned()
        .ok_or_else(|| user(format!("no page {key:?}; list_pages shows what exists")))
}

fn search(store: &Store, input: &Value) -> ToolResult {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct In {
        query: String,
    }
    const MAX_HITS: usize = 60;
    let In { query } = parse(input)?;
    let needle = must_trim(&query, "query")?.to_lowercase();
    let corpus = store.corpus().map_err(Fail::from)?;
    let files = mise_store::render::render(&corpus);
    let mut hits = Vec::new();
    for (path, content) in &files {
        for (i, line) in content.lines().enumerate() {
            if line.to_lowercase().contains(&needle) {
                hits.push(format!("{path}:{}: {line}", i + 1));
            }
        }
    }
    let total = hits.len();
    if total == 0 {
        return Ok(format!("no matches for {query:?}"));
    }
    hits.truncate(MAX_HITS);
    let mut out = hits.join("\n");
    if total > MAX_HITS {
        out.push_str(&format!("\n… {} more matches omitted", total - MAX_HITS));
    }
    Ok(out)
}

// ---------------------------------------------------------------- queue --

pub(crate) fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

fn queue_add(store: &mut Store, ctx: &ToolCtx, input: &Value) -> ToolResult {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct In {
        title: String,
        recipe: Option<String>,
        reason: Option<String>,
        id: Option<String>,
        #[serde(default)]
        someday: bool,
    }
    let a: In = parse(input)?;
    let title = must_trim(&a.title, "title")?;
    let recipe = a.recipe.as_deref().map(slug).transpose()?;
    if let Some(r) = &recipe
        && !store.exists(&DocId::Recipe(r.clone())).map_err(Fail::from)?
    {
        return Err(user(format!("no recipe {r}; recipe_add it first, or omit recipe")));
    }
    let id = match &a.id {
        Some(id) => slug(id)?,
        None => slug(&slugify(&title))?,
    };
    let doc_id = if a.someday { DocId::Someday } else { DocId::Queue };
    let today = ctx.today();
    let mut existed = false;
    store
        .modify::<QueueDoc>(&doc_id, &ctx.msg(&format!("queue add {id}")), ctx.at(), |q| {
            match q.entries.get_mut(&id.to_string()) {
                // Upsert = patch. Age is load-bearing ("21d on the queue"
                // exists so stale entries are noticeable) and a multi-dish
                // entry is a menu — so an existing entry keeps its `added`
                // and its dishes, and only the reason moves. Changing the
                // dish itself is queue_remove + queue_add.
                Some(entry) => {
                    existed = true;
                    if let Some(r) = a.reason.as_deref() {
                        entry.reason = opt_trim(r);
                    }
                }
                None => {
                    q.entries.insert(
                        id.to_string(),
                        QueueEntryDoc {
                            dishes: vec![DishRefDoc {
                                recipe: recipe.as_ref().map(|r| r.to_string()),
                                title,
                            }],
                            reason: a.reason.as_deref().and_then(opt_trim),
                            added: today.to_string(),
                        },
                    );
                }
            }
        })
        .map_err(Fail::from)?;
    let shelf = if a.someday { " (someday)" } else { "" };
    Ok(if existed {
        format!("updated {id}{shelf} — kept its place, age, and dishes")
    } else {
        format!("queued {id}{shelf}")
    })
}

fn queue_remove(store: &mut Store, ctx: &ToolCtx, input: &Value) -> ToolResult {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct In {
        id: String,
        #[serde(default)]
        someday: bool,
    }
    let a: In = parse(input)?;
    let doc_id = if a.someday { DocId::Someday } else { DocId::Queue };
    let existing: QueueDoc = store.get(&doc_id).map_err(Fail::from)?;
    if !existing.entries.contains_key(&a.id) {
        return Err(user(format!("no queue entry {:?}", a.id)));
    }
    store
        .modify::<QueueDoc>(&doc_id, &ctx.msg(&format!("queue remove {}", a.id)), ctx.at(), |q| {
            q.entries.remove(&a.id);
        })
        .map_err(Fail::from)?;
    Ok(format!("removed {}", a.id))
}

// -------------------------------------------------------------- recipes --

#[derive(Deserialize)]

#[serde(deny_unknown_fields)]
struct IngredientIn {
    text: String,
    pantry: Option<String>,
}

fn clean_ingredients(
    raw: Vec<IngredientIn>,
) -> std::result::Result<Vec<IngredientDoc>, Fail> {
    raw.into_iter()
        .map(|i| {
            Ok(IngredientDoc {
                text: must_trim(&i.text, "ingredient text")?,
                pantry: i.pantry.as_deref().map(slug).transpose()?,
            })
        })
        .collect()
}

fn clean_equipment(raw: Vec<String>) -> std::result::Result<Vec<Slug>, Fail> {
    raw.iter().map(|e| slug(e)).collect()
}

fn parse_effort(s: &str) -> std::result::Result<String, Fail> {
    s.parse::<EffortClass>().map(|e| e.to_string()).map_err(user)
}

fn parse_status(s: &str) -> std::result::Result<mise_core::types::RecipeStatus, Fail> {
    s.parse().map_err(user)
}

fn recipe_add(store: &mut Store, ctx: &ToolCtx, input: &Value) -> ToolResult {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct In {
        slug: String,
        title: String,
        servings: Option<u32>,
        effort: Option<String>,
        #[serde(default)]
        tags: BTreeMap<String, String>,
        #[serde(default)]
        equipment: Vec<String>,
        lead_minutes: Option<u32>,
        lead_step: Option<String>,
        #[serde(default)]
        ingredients: Vec<IngredientIn>,
        body: Option<String>,
        status: Option<String>,
        source: Option<String>,
    }
    let a: In = parse(input)?;
    let s = slug(&a.slug)?;
    let status = parse_status(a.status.as_deref().unwrap_or("active"))?;
    if status == mise_core::types::RecipeStatus::Retired {
        return Err(user("a new recipe can be draft or active, not retired"));
    }
    let servings = bounded_servings(a.servings.unwrap_or(2))?;
    let lead = build_lead(a.lead_minutes, a.lead_step.as_deref())?;
    let doc = RecipeDoc {
        schema_version: mise_store::pages::SCHEMA_VERSION,
        title: must_trim(&a.title, "title")?,
        servings,
        effort: parse_effort(a.effort.as_deref().unwrap_or("weekday"))?,
        lead,
        tags: clean_tags(a.tags)?,
        equipment: clean_equipment(a.equipment)?,
        ingredients: clean_ingredients(a.ingredients)?,
        source: a.source.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(String::from),
        status,
        body: a.body.as_deref().unwrap_or("").trim().into(),
    };
    store
        .create_doc(&DocId::Recipe(s.clone()), &doc, &ctx.msg(&format!("recipe add {s}")), ctx.at())
        .map_err(Fail::from)?;
    Ok(format!("added recipe {s}"))
}

fn build_lead(
    minutes: Option<u32>,
    step: Option<&str>,
) -> std::result::Result<Option<LeadTimeDoc>, Fail> {
    match (minutes, step) {
        (Some(0), _) => Err(user("lead_minutes must be at least 1")),
        (Some(m), Some(step)) => Ok(Some(LeadTimeDoc {
            minutes: m,
            act_now_step: must_trim(step, "lead_step")?,
        })),
        (Some(_), None) | (None, Some(_)) => {
            Err(user("lead_minutes and lead_step go together"))
        }
        (None, None) => Ok(None),
    }
}

fn recipe_edit(store: &mut Store, ctx: &ToolCtx, input: &Value) -> ToolResult {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct In {
        slug: String,
        title: Option<String>,
        servings: Option<u32>,
        effort: Option<String>,
        tags: Option<BTreeMap<String, String>>,
        equipment: Option<Vec<String>>,
        lead_minutes: Option<u32>,
        lead_step: Option<String>,
        #[serde(default)]
        clear_lead: bool,
        ingredients: Option<Vec<IngredientIn>>,
        body: Option<String>,
        status: Option<String>,
        source: Option<String>,
    }
    let a: In = parse(input)?;
    let s = slug(&a.slug)?;
    // Validate everything before touching the doc.
    let title = a.title.as_deref().map(|t| must_trim(t, "title")).transpose()?;
    if let Some(v) = a.servings {
        bounded_servings(v)?;
    }
    let effort = a.effort.as_deref().map(parse_effort).transpose()?;
    let tags = a.tags.map(clean_tags).transpose()?;
    let equipment = a.equipment.map(clean_equipment).transpose()?;
    let ingredients = a.ingredients.map(clean_ingredients).transpose()?;
    let lead = build_lead(a.lead_minutes, a.lead_step.as_deref())?;
    if a.clear_lead && lead.is_some() {
        return Err(user("clear_lead contradicts lead_minutes/lead_step"));
    }
    let status = a.status.as_deref().map(parse_status).transpose()?;
    let body = a.body.map(|b| b.trim().to_string());
    let msg = ctx.msg(&format!("recipe edit {s}"));

    store
        .modify::<RecipeDoc>(&DocId::Recipe(s.clone()), &msg, ctx.at(), |r| {
            if let Some(t) = title {
                r.title = t;
            }
            if let Some(v) = a.servings {
                r.servings = v;
            }
            if let Some(e) = effort {
                r.effort = e;
            }
            if let Some(t) = tags {
                r.tags = t;
            }
            if let Some(e) = equipment {
                r.equipment = e;
            }
            if let Some(i) = ingredients {
                r.ingredients = i;
            }
            if a.clear_lead {
                r.lead = None;
            } else if let Some(l) = lead {
                r.lead = Some(l);
            }
            if let Some(st) = status {
                r.status = st;
            }
            if let Some(src) = &a.source {
                let src = src.trim();
                r.source = (!src.is_empty()).then(|| src.to_string());
            }
        })
        .map_err(Fail::from)?;
    // The body goes through the store's char-safe diff splice, not
    // autosurgeon's byte-indexed Text::update.
    if let Some(b) = &body {
        store.update_body(&DocId::Recipe(s.clone()), b, &msg, ctx.at()).map_err(Fail::from)?;
    }
    Ok(format!("updated recipe {s}"))
}

// --------------------------------------------------------------- pantry --

fn pantry_set(store: &mut Store, ctx: &ToolCtx, input: &Value) -> ToolResult {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct In {
        item: String,
        name: Option<String>,
        presence: Option<String>,
        tier: Option<String>,
        bought: Option<String>,
        note: Option<String>,
        location: Option<String>,
    }
    let a: In = parse(input)?;
    let item = slug(&a.item)?;
    let loc = resolve_location(store, &a.location)?;
    if let Some(p) = &a.presence {
        p.parse::<Presence>().map_err(user)?;
    }
    let tier = resolve_tier(store, &loc, a.tier.as_deref())?;
    let bought = a.bought.as_deref().map(|b| parse_date(b, ctx.today())).transpose()?;
    store
        .modify::<PantryDoc>(
            &DocId::Pantry(loc.clone()),
            &ctx.msg(&format!("pantry {loc}: set {item}")),
            ctx.at(),
            |p| {
                let entry = p.items.entry(item.to_string()).or_insert_with(|| PantryItemDoc {
                    name: item.as_str().replace('-', " "),
                    presence: "have".to_string(),
                    bought: None,
                    tier: None,
                    note: None,
                });
                if let Some(n) = a.name.as_deref().and_then(opt_trim) {
                    entry.name = n;
                }
                if let Some(p) = &a.presence {
                    entry.presence = p.clone();
                }
                if let Some(t) = &tier {
                    entry.tier = Some(t.to_string());
                }
                if let Some(b) = &bought {
                    entry.bought = Some(b.to_string());
                }
                if let Some(n) = &a.note {
                    entry.note = opt_trim(n);
                }
            },
        )
        .map_err(Fail::from)?;
    Ok(format!("pantry {loc}: {item} updated"))
}

fn pantry_remove(store: &mut Store, ctx: &ToolCtx, input: &Value) -> ToolResult {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct In {
        item: String,
        location: Option<String>,
    }
    let a: In = parse(input)?;
    let loc = resolve_location(store, &a.location)?;
    let existing: PantryDoc = store.get(&DocId::Pantry(loc.clone())).map_err(Fail::from)?;
    if !existing.items.contains_key(a.item.trim()) {
        return Err(user(format!("no pantry item {:?} at {loc}", a.item)));
    }
    store
        .modify::<PantryDoc>(
            &DocId::Pantry(loc.clone()),
            &ctx.msg(&format!("pantry {loc}: remove {}", a.item)),
            ctx.at(),
            |p| {
                p.items.remove(a.item.trim());
            },
        )
        .map_err(Fail::from)?;
    Ok(format!("pantry {loc}: {} removed", a.item))
}

// ------------------------------------------------------------ equipment --

fn equipment_set(store: &mut Store, ctx: &ToolCtx, input: &Value) -> ToolResult {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct In {
        item: String,
        note: Option<String>,
        location: Option<String>,
    }
    let a: In = parse(input)?;
    let item = slug(&a.item)?;
    let loc = resolve_location(store, &a.location)?;
    store
        .modify::<EquipmentDoc>(
            &DocId::Equipment(loc.clone()),
            &ctx.msg(&format!("equipment {loc}: set {item}")),
            ctx.at(),
            |e| {
                // Entry-and-patch, same contract as pantry_set: only the
                // fields you pass change. An explicit "" clears the note;
                // omitting it keeps what's there.
                let entry = e.items.entry(item.to_string()).or_default();
                if let Some(n) = a.note.as_deref() {
                    *entry = n.trim().to_string();
                }
            },
        )
        .map_err(Fail::from)?;
    Ok(format!("equipment {loc}: {item} recorded"))
}

fn equipment_remove(store: &mut Store, ctx: &ToolCtx, input: &Value) -> ToolResult {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct In {
        item: String,
        location: Option<String>,
    }
    let a: In = parse(input)?;
    let loc = resolve_location(store, &a.location)?;
    let existing: EquipmentDoc = store.get(&DocId::Equipment(loc.clone())).map_err(Fail::from)?;
    if !existing.items.contains_key(a.item.trim()) {
        return Err(user(format!("no equipment {:?} at {loc}", a.item)));
    }
    store
        .modify::<EquipmentDoc>(
            &DocId::Equipment(loc.clone()),
            &ctx.msg(&format!("equipment {loc}: remove {}", a.item)),
            ctx.at(),
            |e| {
                e.items.remove(a.item.trim());
            },
        )
        .map_err(Fail::from)?;
    Ok(format!("equipment {loc}: {} removed", a.item))
}

// --------------------------------------------------------------- fridge --

fn fridge_add(store: &mut Store, ctx: &ToolCtx, input: &Value) -> ToolResult {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct In {
        dish: String,
        servings: u32,
        date: Option<String>,
        freezer: Option<String>,
        location: Option<String>,
    }
    let a: In = parse(input)?;
    let loc = resolve_location(store, &a.location)?;
    let dish = must_trim(&a.dish, "dish")?;
    let servings = bounded_servings(a.servings)?;
    let date = a
        .date
        .as_deref()
        .map(|d| parse_date(d, ctx.today()))
        .transpose()?
        .unwrap_or(ctx.today());
    // Minted, never positional: portion ids are CRDT map keys, and two
    // replicas both picking the lowest free `p1` while apart merge to one
    // surviving portion.
    let assigned = store.mint_id("p").map_err(Fail::from)?;
    store
        .modify::<FridgeDoc>(
            &DocId::Fridge(loc.clone()),
            &ctx.msg(&format!("fridge {loc}: add {dish}")),
            ctx.at(),
            |f| {
                let portions = match &a.freezer {
                    Some(name) => f.freezers.entry(name.trim().to_string()).or_default(),
                    None => &mut f.fridge,
                };
                portions.insert(
                    assigned.clone(),
                    PortionDoc { dish: dish.clone(), servings, date: date.to_string() },
                );
            },
        )
        .map_err(Fail::from)?;
    Ok(format!("fridge {loc}: added {dish} as {assigned} ({servings} servings)"))
}

fn fridge_remove(store: &mut Store, ctx: &ToolCtx, input: &Value) -> ToolResult {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct In {
        id: String,
        freezer: Option<String>,
        location: Option<String>,
    }
    let a: In = parse(input)?;
    let loc = resolve_location(store, &a.location)?;
    let existing: FridgeDoc = store.get(&DocId::Fridge(loc.clone())).map_err(Fail::from)?;
    let there = match &a.freezer {
        Some(name) => existing
            .freezers
            .get(name.trim())
            .is_some_and(|p| p.contains_key(&a.id)),
        None => existing.fridge.contains_key(&a.id),
    };
    if !there {
        return Err(user(format!("no portion {:?} there", a.id)));
    }
    store
        .modify::<FridgeDoc>(
            &DocId::Fridge(loc.clone()),
            &ctx.msg(&format!("fridge {loc}: remove {}", a.id)),
            ctx.at(),
            |f| match &a.freezer {
                Some(name) => {
                    if let Some(portions) = f.freezers.get_mut(name.trim()) {
                        portions.remove(&a.id);
                        if portions.is_empty() {
                            f.freezers.remove(name.trim());
                        }
                    }
                }
                None => {
                    f.fridge.remove(&a.id);
                }
            },
        )
        .map_err(Fail::from)?;
    Ok(format!("fridge {loc}: removed {}", a.id))
}

// ------------------------------------------------------------------ log --

fn log_append(store: &mut Store, ctx: &ToolCtx, input: &Value) -> ToolResult {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct In {
        title: String,
        recipe: Option<String>,
        kind: Option<String>,
        servings: Option<u32>,
        verdict: Option<String>,
        date: Option<String>,
        #[serde(default)]
        tags: BTreeMap<String, String>,
        location: Option<String>,
    }
    let a: In = parse(input)?;
    let loc = resolve_location(store, &a.location)?;
    let kind: CookKind = a.kind.as_deref().unwrap_or("meal").parse().map_err(user)?;
    let recipe = a.recipe.as_deref().map(slug).transpose()?;
    let date = a
        .date
        .as_deref()
        .map(|d| parse_date(d, ctx.today()))
        .transpose()?
        .unwrap_or(ctx.today());
    let mut entry_tags = BTreeMap::new();
    let mut servings_default = None;
    if let Some(r) = &recipe {
        let doc: RecipeDoc = store.get(&DocId::Recipe(r.clone())).map_err(Fail::from)?;
        entry_tags = doc.tags.clone();
        servings_default = Some(doc.servings);
    }
    entry_tags.extend(clean_tags(a.tags)?);
    let servings = bounded_servings(
        a.servings
            .or(servings_default)
            .ok_or_else(|| user("no servings given and no recipe to take a default from"))?,
    )?;
    let entry = LogEntry {
        date,
        kind,
        recipe,
        title: must_trim(&a.title, "title")?,
        location: loc.to_string(),
        servings,
        verdict: a.verdict.as_deref().unwrap_or("fine").trim().to_string(),
        tags: entry_tags,
    };
    store
        .append_log(&entry, &ctx.msg(&format!("log {}", entry.title)), ctx.at())
        .map_err(Fail::from)?;
    Ok(format!("logged: {} on {} at {loc}", entry.title, entry.date))
}

// ------------------------------------------------------------- shopping --

fn shopping_add(store: &mut Store, ctx: &ToolCtx, input: &Value) -> ToolResult {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct In {
        text: String,
        tier: Option<String>,
        id: Option<String>,
    }
    let a: In = parse(input)?;
    let text = must_trim(&a.text, "text")?;
    // The shopping list is corpus-global; tiers come from the active
    // location's shops page.
    let loc = resolve_location(store, &None)?;
    let tier = resolve_tier(store, &loc, a.tier.as_deref())?;
    let requested = a.id.as_deref().map(slug).transpose()?;
    // Minted, never positional — see fridge_add.
    let assigned = match &requested {
        Some(id) => id.to_string(),
        None => store.mint_id("s").map_err(Fail::from)?,
    };
    store
        .modify::<ShoppingDoc>(&DocId::Shopping, &ctx.msg(&format!("shopping add {text}")), ctx.at(), |d| {
            d.items.insert(
                assigned.clone(),
                ShoppingItemDoc {
                    text: text.clone(),
                    tier: tier.as_ref().map(|t| t.to_string()),
                    done: false,
                },
            );
        })
        .map_err(Fail::from)?;
    Ok(format!("shopping: added {text} as {assigned}"))
}

fn shopping_update(store: &mut Store, ctx: &ToolCtx, input: &Value) -> ToolResult {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct In {
        id: String,
        done: Option<bool>,
        #[serde(default)]
        remove: bool,
    }
    let a: In = parse(input)?;
    if !a.remove && a.done.is_none() {
        return Err(user("say what changes: done: true/false, or remove: true"));
    }
    let existing: ShoppingDoc = store.get(&DocId::Shopping).map_err(Fail::from)?;
    if !existing.items.contains_key(a.id.trim()) {
        return Err(user(format!("no shopping item {:?}", a.id)));
    }
    let action = if a.remove { "remove" } else { "update" };
    store
        .modify::<ShoppingDoc>(&DocId::Shopping, &ctx.msg(&format!("shopping {action} {}", a.id)), ctx.at(), |d| {
            if a.remove {
                d.items.remove(a.id.trim());
            } else if let Some(item) = d.items.get_mut(a.id.trim())
                && let Some(done) = a.done
            {
                item.done = done;
            }
        })
        .map_err(Fail::from)?;
    Ok(format!("shopping: {action}d {}", a.id))
}

// -------------------------------------------------------- steering/facts --

#[derive(Deserialize)]

#[serde(deny_unknown_fields)]
struct KvIn {
    key: String,
    #[serde(alias = "fact")]
    note: Option<String>,
}

/// Parsed kv input plus the provenance action, shared by both kv pages.
fn kv_parts(input: &Value) -> std::result::Result<(String, Option<String>, &'static str), Fail> {
    let a: KvIn = parse(input)?;
    let key = must_trim(&a.key, "key")?;
    let value = a.note.as_deref().and_then(opt_trim);
    let action = if value.is_some() { "set" } else { "clear" };
    Ok((key, value, action))
}

fn kv_apply(entries: &mut BTreeMap<String, String>, key: &str, value: &Option<String>) {
    match value {
        Some(v) => {
            entries.insert(key.to_string(), v.clone());
        }
        None => {
            entries.remove(key);
        }
    }
}

fn steering_set(store: &mut Store, ctx: &ToolCtx, input: &Value) -> ToolResult {
    let (key, value, action) = kv_parts(input)?;
    store
        .modify::<SteeringDoc>(&DocId::Steering, &ctx.msg(&format!("steering {action} {key}")), ctx.at(), |d| {
            kv_apply(&mut d.entries, &key, &value);
        })
        .map_err(Fail::from)?;
    Ok(format!("steering: {action} {key}"))
}

fn facts_set(store: &mut Store, ctx: &ToolCtx, input: &Value) -> ToolResult {
    let (key, value, action) = kv_parts(input)?;
    store
        .modify::<FactsDoc>(&DocId::Facts, &ctx.msg(&format!("facts {action} {key}")), ctx.at(), |d| {
            kv_apply(&mut d.facts, &key, &value);
        })
        .map_err(Fail::from)?;
    Ok(format!("facts: {action} {key}"))
}
