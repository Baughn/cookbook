//! Tool logic against a real temp store, scripted inputs only — no model
//! anywhere. Every tool gets its happy path and its is_error path.

use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use mise_assistant::tools::{ToolCtx, execute};
use mise_assistant::turn::ToolCall;
use mise_core::types::Slug;
use mise_store::pages::{FridgeDoc, QueueDoc, RecipeDoc, ShoppingDoc, SteeringDoc};
use mise_store::{DocId, Store};
use serde_json::{Value, json};

fn now() -> DateTime {
    DateTime::constant(2026, 7, 29, 12, 0, 0, 0)
}

fn ctx() -> ToolCtx {
    ToolCtx { now: now().to_zoned(TimeZone::UTC).unwrap(), provenance: "planning thread".into() }
}

fn fresh() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(&dir.path().join("corpus"), &Slug::new("home").unwrap(), 2, jiff::Timestamp::UNIX_EPOCH).unwrap();
    (dir, store)
}

fn run(store: &mut Store, name: &str, input: Value) -> (String, bool) {
    let call = ToolCall { id: "t1".into(), name: name.into(), input };
    let out = execute(store, &ctx(), &call).unwrap();
    assert_eq!(out.tool_use_id, "t1");
    (out.content, out.is_error)
}

fn ok(store: &mut Store, name: &str, input: Value) -> String {
    let (content, is_error) = run(store, name, input);
    assert!(!is_error, "{name} failed: {content}");
    content
}

fn err(store: &mut Store, name: &str, input: Value) -> String {
    let (content, is_error) = run(store, name, input);
    assert!(is_error, "{name} unexpectedly succeeded: {content}");
    content
}

fn seed_recipe(store: &mut Store) {
    ok(
        store,
        "recipe_add",
        json!({
            "slug": "mapo-tofu",
            "title": "Mapo tofu",
            "servings": 4,
            "tags": {"cuisine": "sichuan", "protein": "pork"},
            "equipment": ["wok"],
            "ingredients": [
                {"text": "1 block silken tofu"},
                {"text": "doubanjiang", "pantry": "doubanjiang"},
            ],
            "body": "Fry the paste, add stock, slide in the tofu.",
        }),
    );
}

#[test]
fn recipe_add_edit_and_errors() {
    let (_dir, mut store) = fresh();
    seed_recipe(&mut store);
    let doc: RecipeDoc = store.get(&DocId::Recipe(Slug::new("mapo-tofu").unwrap())).unwrap();
    assert_eq!(doc.title, "Mapo tofu");
    assert_eq!(doc.ingredients.len(), 2);
    assert_eq!(doc.ingredients[1].pantry.as_ref().map(|s| s.as_str()), Some("doubanjiang"));

    // Duplicate slug is the model's problem, not a crash.
    assert!(err(&mut store, "recipe_add", json!({"slug": "mapo-tofu", "title": "Again"}))
        .contains("already exists"));
    err(&mut store, "recipe_add", json!({"slug": "Bad Slug", "title": "x"}));
    err(&mut store, "recipe_add", json!({"slug": "zero", "title": "x", "servings": 0}));
    err(
        &mut store,
        "recipe_add",
        json!({"slug": "half-lead", "title": "x", "lead_minutes": 30}),
    );

    ok(
        &mut store,
        "recipe_edit",
        json!({
            "slug": "mapo-tofu",
            "servings": 6,
            "tags": {"cuisine": "sichuan"},
            "lead_minutes": 45, "lead_step": "press the tofu",
            "body": "Fry the paste, add stock, slide in the tofu. Rest five minutes.",
        }),
    );
    let doc: RecipeDoc = store.get(&DocId::Recipe(Slug::new("mapo-tofu").unwrap())).unwrap();
    assert_eq!(doc.servings, 6);
    assert_eq!(doc.tags.len(), 1, "tags replace wholesale");
    assert_eq!(doc.lead.as_ref().unwrap().minutes, 45);
    assert!(doc.body.as_str().ends_with("Rest five minutes."));

    ok(&mut store, "recipe_edit", json!({"slug": "mapo-tofu", "clear_lead": true}));
    let doc: RecipeDoc = store.get(&DocId::Recipe(Slug::new("mapo-tofu").unwrap())).unwrap();
    assert!(doc.lead.is_none());

    err(&mut store, "recipe_edit", json!({"slug": "no-such-recipe", "servings": 2}));
    err(
        &mut store,
        "recipe_edit",
        json!({"slug": "mapo-tofu", "clear_lead": true, "lead_minutes": 5, "lead_step": "x"}),
    );
}

#[test]
fn queue_flow_and_status() {
    let (_dir, mut store) = fresh();
    seed_recipe(&mut store);

    // Linking a nonexistent recipe is refused; ideas are fine.
    err(&mut store, "queue_add", json!({"title": "Duck curry", "recipe": "duck-curry"}));
    ok(&mut store, "queue_add", json!({"title": "Something with duck"}));
    ok(
        &mut store,
        "queue_add",
        json!({"title": "Mapo tofu", "recipe": "mapo-tofu", "reason": "craving"}),
    );
    ok(&mut store, "queue_add", json!({"title": "Croissants", "someday": true}));

    let queue: QueueDoc = store.get(&DocId::Queue).unwrap();
    assert!(queue.entries.contains_key("something-with-duck"), "id slugified from title");

    let status = ok(&mut store, "queue_status", json!({}));
    assert!(status.contains("Queue — home (cooking for 2)"), "{status}");
    assert!(status.contains("missing equipment here: wok"), "{status}");
    assert!(status.contains("why: craving"), "{status}");
    assert!(status.contains("Someday shelf"), "{status}");

    // With the wok recorded, readiness moves on to shopping for doubanjiang
    // (unknown tier — the pantry has no such item).
    ok(&mut store, "equipment_set", json!({"item": "wok"}));
    let status = ok(&mut store, "queue_status", json!({}));
    assert!(status.contains("shop — source unknown: doubanjiang"), "{status}");
    assert!(status.contains("1 unlinked ingredient"), "{status}");

    ok(&mut store, "queue_remove", json!({"id": "something-with-duck"}));
    err(&mut store, "queue_remove", json!({"id": "something-with-duck"}));
    err(&mut store, "queue_remove", json!({"id": "croissants"}));
    ok(&mut store, "queue_remove", json!({"id": "croissants", "someday": true}));
}

#[test]
fn pantry_and_equipment() {
    let (_dir, mut store) = fresh();

    ok(&mut store, "pantry_set", json!({"item": "doubanjiang", "tier": "town"}));
    let status = ok(&mut store, "read_page", json!({"path": "locations/home/pantry"}));
    assert!(status.contains("| doubanjiang | doubanjiang | have |"), "{status}");

    ok(
        &mut store,
        "pantry_set",
        json!({"item": "doubanjiang", "presence": "low", "bought": "today", "note": "the good jar"}),
    );
    let page = ok(&mut store, "read_page", json!({"path": "locations/home/pantry.md"}));
    assert!(page.contains("| low | 2026-07-29 | town | the good jar |"), "{page}");

    err(&mut store, "pantry_set", json!({"item": "x", "presence": "plenty"}));
    err(&mut store, "pantry_set", json!({"item": "x", "location": "cottage"}));
    err(&mut store, "pantry_remove", json!({"item": "wakame"}));
    ok(&mut store, "pantry_remove", json!({"item": "doubanjiang"}));

    ok(&mut store, "equipment_set", json!({"item": "stand-mixer", "note": "borrowed"}));
    err(&mut store, "equipment_remove", json!({"item": "wok"}));
    ok(&mut store, "equipment_remove", json!({"item": "stand-mixer"}));
}

/// The assigned id from an "added … as <id>" tool reply.
fn id_from(msg: &str) -> String {
    msg.rsplit_once(" as ").unwrap().1.split_whitespace().next().unwrap().to_string()
}

#[test]
fn fridge_flow() {
    let (_dir, mut store) = fresh();

    let msg = ok(&mut store, "fridge_add", json!({"dish": "Dal", "servings": 4}));
    let p_dal = id_from(&msg);
    let msg = ok(&mut store, "fridge_add", json!({"dish": "Stock", "servings": 2, "freezer": "basement"}));
    let p_stock = id_from(&msg);
    assert_ne!(p_dal, p_stock, "ids are minted, never reused — even across compartments");

    let doc: FridgeDoc = store.get(&DocId::Fridge(Slug::new("home").unwrap())).unwrap();
    assert_eq!(doc.fridge[&p_dal].dish, "Dal");
    assert_eq!(doc.freezers["basement"][&p_stock].dish, "Stock");

    // Absurd counts stop at the tool boundary: one persisted 100M-serving
    // portion syncs everywhere and used to panic every queue read.
    err(&mut store, "fridge_add", json!({"dish": "Gruel", "servings": 100_000_000}));
    err(&mut store, "fridge_add", json!({"dish": "Air", "servings": 0}));

    err(&mut store, "fridge_remove", json!({"id": p_stock, "freezer": "attic"}));
    err(&mut store, "fridge_remove", json!({"id": "p9"}));
    ok(&mut store, "fridge_remove", json!({"id": p_stock, "freezer": "basement"}));
    let doc: FridgeDoc = store.get(&DocId::Fridge(Slug::new("home").unwrap())).unwrap();
    assert!(doc.freezers.is_empty(), "empty freezer compartment pruned");
}

#[test]
fn log_inherits_from_recipe() {
    let (_dir, mut store) = fresh();
    seed_recipe(&mut store);

    // No servings, no recipe: nothing to default from.
    err(&mut store, "log_append", json!({"title": "Mystery stew"}));

    ok(
        &mut store,
        "log_append",
        json!({"title": "Mapo tofu", "recipe": "mapo-tofu", "verdict": "great", "tags": {"format": "braise"}}),
    );
    let log = store.log_entries().unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].servings, 4, "servings from the recipe");
    assert_eq!(log[0].date, now().date(), "date defaults to today");
    assert_eq!(log[0].tags["cuisine"], "sichuan", "tags inherited");
    assert_eq!(log[0].tags["format"], "braise", "extra tags merged");

    err(&mut store, "log_append", json!({"title": "x", "recipe": "nope", "servings": 2}));
    err(&mut store, "log_append", json!({"title": "x", "servings": 2, "kind": "banquet"}));
}

#[test]
fn shopping_flow() {
    let (_dir, mut store) = fresh();

    let msg = ok(&mut store, "shopping_add", json!({"text": "duck legs", "tier": "butcher"}));
    let s_duck = id_from(&msg);
    ok(&mut store, "shopping_add", json!({"text": "wakame", "id": "wakame"}));

    ok(&mut store, "shopping_update", json!({"id": s_duck, "done": true}));
    let doc: ShoppingDoc = store.get(&DocId::Shopping).unwrap();
    assert!(doc.items[&s_duck].done);

    ok(&mut store, "shopping_update", json!({"id": "wakame", "remove": true}));
    err(&mut store, "shopping_update", json!({"id": "wakame", "done": true}));
}

#[test]
fn steering_and_facts() {
    let (_dir, mut store) = fresh();

    ok(&mut store, "steering_set", json!({"key": "yeast", "note": "beyond sourdough"}));
    ok(&mut store, "facts_set", json!({"key": "grinder", "fact": "hates cleaning the meat grinder"}));
    let steering: SteeringDoc = store.get(&DocId::Steering).unwrap();
    assert_eq!(steering.entries["yeast"], "beyond sourdough");

    // The fact alias works, and clearing removes.
    ok(&mut store, "steering_set", json!({"key": "yeast"}));
    let steering: SteeringDoc = store.get(&DocId::Steering).unwrap();
    assert!(steering.entries.is_empty());
    err(&mut store, "steering_set", json!({"key": "   "}));
}

#[test]
fn reads_and_search() {
    let (_dir, mut store) = fresh();
    seed_recipe(&mut store);

    let pages = ok(&mut store, "list_pages", json!({}));
    assert!(
        pages.contains("recipes/mapo-tofu.md — Mapo tofu [cuisine=sichuan;protein=pork] (weekday)"),
        "{pages}"
    );
    assert!(pages.contains("locations/home/shops.md"), "{pages}");

    let page = ok(&mut store, "read_page", json!({"path": "recipes/mapo-tofu"}));
    assert!(page.contains("# Mapo tofu"), "{page}");
    err(&mut store, "read_page", json!({"path": "recipes/duck-curry"}));

    let hits = ok(&mut store, "search", json!({"query": "DOUBANJIANG"}));
    assert!(hits.contains("recipes/mapo-tofu.md:"), "case-insensitive: {hits}");
    let none = ok(&mut store, "search", json!({"query": "zzznothing"}));
    assert!(none.contains("no matches"), "{none}");
}

#[test]
fn bad_input_and_unknown_tools_are_model_problems() {
    let (_dir, mut store) = fresh();
    assert!(err(&mut store, "queue_add", json!({"reason": 42})).contains("bad tool input"));
    assert!(err(&mut store, "no_such_tool", json!({})).contains("no such tool"));
}

/// Regression: the first live debrief crashed with "index 95 is out of
/// bounds" — a non-ASCII body fed through autosurgeon's byte-indexed
/// Text::update. Bodies now go through Store::update_body.
#[test]
fn recipe_edit_takes_non_ascii_bodies() {
    let (_dir, mut store) = fresh();
    seed_recipe(&mut store);
    let body = "Fry the paste, add stock, slide in the tofu. Rest five minutes.\n\n\
        Notes: double the doubanjiang — 2× is the move; six servings, not four.";
    ok(&mut store, "recipe_edit", json!({"slug": "mapo-tofu", "body": body}));
    let doc: RecipeDoc = store.get(&DocId::Recipe(Slug::new("mapo-tofu").unwrap())).unwrap();
    assert_eq!(doc.body.as_str(), body);
}

/// The charter's basement scenario, through the real tool path: two devices
/// each add items while apart. Positional ids (both sides minting `s1`)
/// collide on merge — Automerge resolves the two concurrent puts at one key
/// to one winner, and the loser's item vanishes from every replica and from
/// the export, silently.
#[test]
fn concurrent_adds_on_two_devices_both_survive_the_merge() {
    use mise_store::sync::Peer;
    fn sync(a: &mut Store, b: &mut Store) {
        let mut pa = Peer::start(a, true).unwrap();
        let mut pb = Peer::start(b, false).unwrap();
        let mut msg = pa.initial_round(a).unwrap();
        loop {
            let reply = match pb.handle(b, &msg).unwrap() {
                Some(r) => r,
                None => return,
            };
            match pa.handle(a, &reply).unwrap() {
                Some(next) => msg = next,
                None => return,
            }
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let mut a = Store::create(
        &dir.path().join("a"),
        &Slug::new("home").unwrap(),
        2,
        jiff::Timestamp::UNIX_EPOCH,
    )
    .unwrap();
    let mut b = Store::create_bare(&dir.path().join("b")).unwrap();
    sync(&mut a, &mut b);

    ok(&mut a, "shopping_add", json!({"text": "milk"}));
    ok(&mut b, "shopping_add", json!({"text": "eggs"}));
    ok(&mut a, "fridge_add", json!({"dish": "Dal", "servings": 2}));
    ok(&mut b, "fridge_add", json!({"dish": "Stock", "servings": 3}));

    sync(&mut a, &mut b);
    let shopping: ShoppingDoc = a.get(&DocId::Shopping).unwrap();
    let texts: Vec<_> = shopping.items.values().map(|i| i.text.as_str()).collect();
    assert_eq!(shopping.items.len(), 2, "a shopping id collision swallowed an item: {texts:?}");
    let fridge: FridgeDoc = a.get(&DocId::Fridge(Slug::new("home").unwrap())).unwrap();
    let dishes: Vec<_> = fridge.fridge.values().map(|p| p.dish.as_str()).collect();
    assert_eq!(fridge.fridge.len(), 2, "a fridge id collision swallowed a portion: {dishes:?}");
}

/// Change messages are immutable and replicate to every device, and their
/// action text embeds model words — which may in turn quote a fetched
/// page. One rule at the funnel, whatever the source: a history line is
/// one bounded line, no control characters, so nothing can forge a
/// second "ui:"-looking entry or bloat every replica's history.
#[test]
fn change_messages_are_one_bounded_line() {
    let (_dir, mut store) = fresh();

    let hostile = format!("milk\nui: forged history line{}", "!".repeat(10_000));
    ok(&mut store, "shopping_add", json!({"text": hostile}));

    let changes = store.history(&DocId::Shopping).unwrap();
    let message = &changes.last().unwrap().message;
    assert!(!message.contains('\n'), "multi-line history: {message:?}");
    assert!(!message.chars().any(char::is_control), "{message:?}");
    assert!(message.chars().count() <= 200, "unbounded history line: {}", message.len());
    assert!(message.starts_with("planning thread: "), "provenance survives: {message:?}");
}
