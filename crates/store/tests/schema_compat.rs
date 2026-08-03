//! The schema-compatibility guard: frozen bytes of a schema-version-1 doc
//! of every kind, and the promise that they stay readable forever.
//!
//! Why bytes and not a struct declaration. The corpus is live and is never
//! reset, and Automerge history only grows: `Store::revert` forks at a
//! historical hash and hydrates *that* state, and sync applies changes from
//! peers still running older builds. So a forward converter has no fixed
//! point — it cannot rewrite the past it has to read, and an un-upgraded
//! phone reintroduces the old shape the moment it syncs. The policy
//! (docs/implementation.md → *Schema changes*) is therefore a **permanent
//! tolerant hydrator** per shape change, and these files are what proves one
//! is still there.
//!
//! ## These files are frozen
//!
//! `tests/fixtures/schema-v1/*.change` are real Automerge changes written by
//! the build that shipped schema version 1. They are the artifact, not a
//! convenience: regenerating them against a newer struct would quietly turn
//! this test into a tautology. The writer below refuses to overwrite a file
//! that exists, so it can only ever populate a *new* version directory.
//!
//! When a doc shape changes, it does not change here. Add
//! `tests/fixtures/schema-v<n>/`, extend the manifest, and leave every
//! earlier version in place — a version's fixtures are deleted only when the
//! hydrator that reads them is, which is never.

use std::path::{Path, PathBuf};

use automerge::transaction::CommitOptions;
use automerge::{ActorId, AutoCommit, Change};
use autosurgeon::{Reconcile, reconcile};
use mise_core::types::{RecipeStatus, Slug};
use mise_store::pages::{
    DishRefDoc, EquipmentDoc, FactsDoc, FridgeDoc, IngredientDoc, LocationMeta, PantryDoc,
    PantryItemDoc, PortionDoc, QueueDoc, QueueEntryDoc, RecipeDoc, ShoppingDoc, ShoppingItemDoc,
    ShopsDoc, StateDoc, SteeringDoc, TechniqueDoc, TierDoc,
};
use mise_store::{DocId, Store};

/// Set this to write fixture files that do not exist yet. Existing files are
/// never touched; see the module docs.
const WRITE_ENV: &str = "MISE_WRITE_FIXTURES";

const VERSION_DIR: &str = "schema-v1";

fn slug(s: &str) -> Slug {
    Slug::new(s).unwrap()
}

fn ts(secs: i64) -> jiff::Timestamp {
    jiff::Timestamp::from_second(secs).unwrap()
}

/// Every doc kind, with *every* distinctive value its v1 fixture carries.
/// The assertion is the charter's own words — everything in the store is
/// legible somewhere in the export — pinned to content that predates the
/// current shapes. One marker per doc would only catch a shape change that
/// happened to drop that one field; this catches any of them.
///
/// A hydrator that answers `Default::default()` for an old shape passes
/// "it hydrates" and fails here, which is the point.
fn manifest() -> Vec<(DocId, &'static [&'static str])> {
    vec![
        (DocId::State, &["home", "7"]),
        (DocId::Queue, &["Fixture stew", "fixture", "2026-01-01"]),
        (DocId::Someday, &["Fixture terrine", "fixture", "2026-01-01"]),
        (DocId::Shopping, &["fixture salt", "shop"]),
        (DocId::Steering, &["rotation", "Fixture steering note"]),
        (DocId::Facts, &["oven", "Fixture standing fact"]),
        (
            DocId::Pantry(slug("home")),
            &["miso", "Fixture miso", "have", "2026-01-02", "shop", "back of the fridge"],
        ),
        (DocId::Equipment(slug("home")), &["wok", "Fixture wok note"]),
        (
            DocId::Shops(slug("home")),
            &["shop", "Fixture market", "town", "Fixture town"],
        ),
        (
            DocId::Fridge(slug("home")),
            &["Fixture dal", "2026-01-03", "chest", "Fixture ragu", "2026-01-04"],
        ),
        (
            DocId::Recipe(slug("fixture-recipe")),
            &[
                "Fixture recipe",
                "weekday",
                "cuisine",
                "fixture",
                "wok",
                "200 g fixture miso",
                "miso",
                "https://example.com/fixture",
                "active",
                "Cook it the fixture way.",
            ],
        ),
        (
            DocId::Technique(slug("fixture-technique")),
            &["Fixture technique", "skill", "fixture", "Hold the knife like so."],
        ),
    ]
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(VERSION_DIR)
}

/// `location/home/pantry` → `location-home-pantry.change`.
fn fixture_path(id: &DocId) -> PathBuf {
    fixture_dir().join(format!("{}.change", id.to_string().replace('/', "-")))
}

// ------------------------------------------------------------- injection --

/// Put a doc into the store as a single change, the way a fresh replica
/// receives one over sync. The store's own change-append path is
/// crate-private, so this goes in through SQL — the same shape
/// `v1_corpus_migrates_on_open` uses to fabricate an old database.
fn inject(root: &Path, id: &DocId, bytes: &[u8]) {
    let change = Change::from_bytes(bytes.to_vec()).expect("fixture is a valid change");
    let hash: String = change.hash().0.iter().map(|b| format!("{b:02x}")).collect();
    let conn = rusqlite::Connection::open(root.join("mise.db")).unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO docs (id, kind) VALUES (?1, ?2)",
        rusqlite::params![id.to_string(), id.kind()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO doc_changes (doc_id, seq, hash, change) VALUES (?1, 1, ?2, ?3)",
        rusqlite::params![id.to_string(), hash, bytes],
    )
    .unwrap();
}

/// The frozen change, loaded as a bare Automerge doc — no store around it.
fn doc_from_fixture(id: &DocId) -> AutoCommit {
    let bytes = std::fs::read(fixture_path(id)).unwrap();
    let mut doc = AutoCommit::new();
    doc.apply_changes([Change::from_bytes(bytes).unwrap()]).unwrap();
    doc
}

/// A store holding nothing but the frozen v1 docs.
fn store_of_frozen_v1(root: &Path) -> Store {
    write_missing_fixtures();
    drop(Store::create_bare(root).unwrap());
    for (id, _) in manifest() {
        let path = fixture_path(&id);
        let bytes = std::fs::read(&path).unwrap_or_else(|e| {
            panic!(
                "missing frozen fixture {}: {e}\n\
                 If this version directory is new, populate it with {WRITE_ENV}=1; \
                 existing fixtures are never regenerated.",
                path.display(),
            )
        });
        inject(root, &id, &bytes);
    }
    Store::open(root).unwrap()
}

// ------------------------------------------------------------------ test --

#[test]
fn frozen_v1_docs_hydrate_render_and_revert() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    let mut store = store_of_frozen_v1(&root);

    // Every doc hydrates, and hydrates *together* — `corpus()` reads all of
    // them, so one unreadable doc would take the whole read path down.
    let corpus = store.corpus().unwrap();
    assert_eq!(corpus.recipes.len(), 1);
    assert_eq!(corpus.locations.len(), 1);

    // The version stamp is legible from the bytes themselves, which is what
    // a hydrator has to work from when presence alone can't decide.
    for (id, _) in manifest() {
        let doc = doc_from_fixture(&id);
        assert_eq!(
            mise_store::pages::schema_version_at(&doc, &automerge::ROOT),
            1,
            "{id} is a schema-version-1 fixture",
        );
    }
    assert_eq!(
        mise_store::pages::schema_version_at(&AutoCommit::new(), &automerge::ROOT),
        0,
        "an unstamped doc reads as older than everything",
    );

    // Everything renders, and the v1 content is still in it.
    store.export("test: frozen v1 corpus").unwrap();
    assert_markers_survived(&store, "after loading v1 bytes");

    // An ordinary new-build edit lands on a v1 doc and reads back, with the
    // fields it didn't touch intact.
    let id = DocId::Recipe(slug("fixture-recipe"));
    store
        .modify::<RecipeDoc>(&id, "test: edit a v1 recipe", ts(100), |r| {
            r.title = "Fixture recipe, revised".into();
        })
        .unwrap();
    let edited: RecipeDoc = store.get(&id).unwrap();
    assert_eq!(edited.title, "Fixture recipe, revised");
    assert_eq!(edited.servings, 4, "an untouched v1 field survives the edit");
    assert_eq!(edited.source.as_deref(), Some("https://example.com/fixture"));

    // The property a forward converter can never provide: revert forks at
    // the v1 head and hydrates *that* state. Doing it for every kind also
    // exercises the write-back, since revert reconciles the hydrated value
    // through the current shape.
    for (id, _) in manifest() {
        let history = store.history(&id).unwrap();
        store
            .revert(&id, &history[0].hash, "test: revert to the v1 head", ts(200))
            .unwrap_or_else(|e| panic!("reverting {id} to its v1 head: {e}"));
    }
    store.export("test: reverted to v1").unwrap();
    assert_markers_survived(&store, "after reverting to the v1 head");

    let back: RecipeDoc = store.get(&id).unwrap();
    assert_eq!(back.title, "Fixture recipe");
}

/// #9: `schema_version` is a write-path invariant, not a value a writer must
/// remember to set. A doc carrying a stale stamp — an older build, a peer on a
/// future build, or a revert restoring a historical value — is brought current
/// by the next write: `Store::modify` stamps *after* the closure, so no
/// new-shape bytes ever persist under an old version, and a revert cannot carry
/// an old stamp forward over current-shape bytes.
#[test]
fn every_write_stamps_the_current_schema_version() {
    use mise_store::pages::SCHEMA_VERSION;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    let mut store = Store::create(&root, &slug("home"), 2, ts(0)).unwrap();

    // Plant a deliberately stale stamp. `create_doc` reconciles verbatim (it is
    // not a mutation), which is exactly how a future-build peer could deliver
    // one; `modify` can no longer even write a non-current stamp.
    let id = DocId::Recipe(slug("stale"));
    let stale = RecipeDoc {
        schema_version: 99,
        title: "Stale".into(),
        servings: 3,
        effort: "weekday".into(),
        lead: None,
        tags: Default::default(),
        equipment: vec![],
        ingredients: vec![],
        source: None,
        status: RecipeStatus::Active,
        body: "".into(),
    };
    store.create_doc(&id, &stale, "test: stale stamp", ts(1)).unwrap();
    assert_eq!(store.get::<RecipeDoc>(&id).unwrap().schema_version, 99, "created verbatim");

    // Any ordinary edit brings it current.
    store
        .modify::<RecipeDoc>(&id, "test: edit", ts(2), |r| r.title = "Renamed".into())
        .unwrap();
    assert_eq!(store.get::<RecipeDoc>(&id).unwrap().schema_version, SCHEMA_VERSION);

    // Reverting to the stale head restores the old *content* but not the stamp.
    let stale_head = store.history(&id).unwrap()[0].hash.clone();
    store.revert(&id, &stale_head, "test: revert to stale head", ts(3)).unwrap();
    let back = store.get::<RecipeDoc>(&id).unwrap();
    assert_eq!(back.title, "Stale", "content reverted");
    assert_eq!(back.schema_version, SCHEMA_VERSION, "but the stamp stays current");
}

fn assert_markers_survived(store: &Store, when: &str) {
    for (id, markers) in manifest() {
        let path = store.export_dir().join(id.export_path());
        let rendered = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{when}: reading {}: {e}", path.display()));
        for marker in markers {
            assert!(
                rendered.contains(*marker),
                "{when}: {id} lost its v1 content — no {marker:?} in:\n{rendered}",
            );
        }
    }
}

// ------------------------------------------------------------- the writer --

/// Encode one doc as a single change with a pinned actor and timestamp, so
/// the bytes are reproducible rather than salted with a random actor id.
fn encode<T: Reconcile>(actor: u8, value: &T) -> Vec<u8> {
    let mut doc = AutoCommit::new();
    doc.set_actor(ActorId::from(vec![actor; 16]));
    reconcile(&mut doc, value).unwrap();
    doc.commit_with(CommitOptions::default().with_message("init: fixture").with_time(0));
    doc.get_last_local_change().expect("reconcile wrote a change").raw_bytes().to_vec()
}

/// Write any fixture file that does not exist yet, and only then. See the
/// module docs: an existing fixture is the historical artifact, and
/// re-encoding it against today's structs would make the test assert
/// nothing.
fn write_missing_fixtures() {
    if std::env::var(WRITE_ENV).is_err() {
        return;
    }
    std::fs::create_dir_all(fixture_dir()).unwrap();
    for (id, bytes) in v1_values() {
        let path = fixture_path(&id);
        if path.exists() {
            continue;
        }
        std::fs::write(&path, &bytes).unwrap();
        eprintln!("wrote {}", path.display());
    }
}

/// The v1 content of every doc. Only reachable from the writer — the test
/// itself reads bytes off disk and never sees these structs, which is what
/// keeps it honest once the shapes move on.
fn v1_values() -> Vec<(DocId, Vec<u8>)> {
    use std::collections::BTreeMap;

    let kv = |k: &str, v: &str| BTreeMap::from([(k.to_string(), v.to_string())]);
    let entry = |title: &str| QueueEntryDoc {
        dishes: vec![DishRefDoc { recipe: None, title: title.into() }],
        reason: Some("fixture".into()),
        added: "2026-01-01".into(),
    };

    vec![
        (
            DocId::State,
            encode(
                1,
                &StateDoc {
                    schema_version: 1,
                    active_location: "home".into(),
                    locations: BTreeMap::from([(
                        "home".to_string(),
                        LocationMeta { headcount: 7 },
                    )]),
                },
            ),
        ),
        (
            DocId::Queue,
            encode(
                2,
                &QueueDoc {
                    schema_version: 1,
                    entries: BTreeMap::from([("fixture-stew".to_string(), entry("Fixture stew"))]),
                },
            ),
        ),
        (
            DocId::Someday,
            encode(
                3,
                &QueueDoc {
                    schema_version: 1,
                    entries: BTreeMap::from([(
                        "fixture-terrine".to_string(),
                        entry("Fixture terrine"),
                    )]),
                },
            ),
        ),
        (
            DocId::Shopping,
            encode(
                4,
                &ShoppingDoc {
                    schema_version: 1,
                    items: BTreeMap::from([(
                        "fixture-salt".to_string(),
                        ShoppingItemDoc {
                            text: "fixture salt".into(),
                            tier: Some("shop".into()),
                            done: false,
                        },
                    )]),
                },
            ),
        ),
        (
            DocId::Steering,
            encode(
                5,
                &SteeringDoc {
                    schema_version: 1,
                    entries: kv("rotation", "Fixture steering note"),
                },
            ),
        ),
        (
            DocId::Facts,
            encode(6, &FactsDoc { schema_version: 1, facts: kv("oven", "Fixture standing fact") }),
        ),
        (
            DocId::Pantry(slug("home")),
            encode(
                7,
                &PantryDoc {
                    schema_version: 1,
                    items: BTreeMap::from([(
                        "miso".to_string(),
                        PantryItemDoc {
                            name: "Fixture miso".into(),
                            presence: "have".into(),
                            bought: Some("2026-01-02".into()),
                            tier: Some("shop".into()),
                            note: Some("back of the fridge".into()),
                        },
                    )]),
                },
            ),
        ),
        (
            DocId::Equipment(slug("home")),
            encode(
                8,
                &EquipmentDoc { schema_version: 1, items: kv("wok", "Fixture wok note") },
            ),
        ),
        (
            DocId::Shops(slug("home")),
            encode(
                9,
                &ShopsDoc {
                    schema_version: 1,
                    tiers: vec![
                        TierDoc { id: "shop".into(), name: "Fixture market".into() },
                        TierDoc { id: "town".into(), name: "Fixture town".into() },
                    ],
                },
            ),
        ),
        (
            DocId::Fridge(slug("home")),
            encode(
                10,
                &FridgeDoc {
                    schema_version: 1,
                    fridge: BTreeMap::from([(
                        "dal".to_string(),
                        PortionDoc {
                            dish: "Fixture dal".into(),
                            servings: 2,
                            date: "2026-01-03".into(),
                        },
                    )]),
                    freezers: BTreeMap::from([(
                        "chest".to_string(),
                        BTreeMap::from([(
                            "ragu".to_string(),
                            PortionDoc {
                                dish: "Fixture ragu".into(),
                                servings: 4,
                                date: "2026-01-04".into(),
                            },
                        )]),
                    )]),
                },
            ),
        ),
        (
            DocId::Recipe(slug("fixture-recipe")),
            encode(
                11,
                &RecipeDoc {
                    schema_version: 1,
                    title: "Fixture recipe".into(),
                    servings: 4,
                    effort: "weekday".into(),
                    lead: None,
                    tags: kv("cuisine", "fixture"),
                    equipment: vec![slug("wok")],
                    ingredients: vec![IngredientDoc {
                        text: "200 g fixture miso".into(),
                        pantry: Some(slug("miso")),
                    }],
                    source: Some("https://example.com/fixture".into()),
                    status: RecipeStatus::Active,
                    body: "Cook it the fixture way.".into(),
                },
            ),
        ),
        (
            DocId::Technique(slug("fixture-technique")),
            encode(
                12,
                &TechniqueDoc {
                    schema_version: 1,
                    title: "Fixture technique".into(),
                    tags: kv("skill", "fixture"),
                    body: "Hold the knife like so.".into(),
                },
            ),
        ),
    ]
}

/// The hydrators are tolerant the way the schema policy demands: values
/// this build cannot interpret — a newer build's status vocabulary, a
/// peer's garbage links — degrade on hydration instead of taking down every
/// read, and nothing out-of-vocabulary can reach the render layer, which is
/// what killed the frontmatter-injection class (a status of
/// "x\n---\ntitle: forged" used to render as forged frontmatter).
#[test]
fn out_of_vocabulary_recipe_values_degrade_instead_of_poisoning_reads() {
    use autosurgeon::Text;

    // The loose v1-era shape: everything stringly, as an old build (or a
    // hand-rolled peer) could write it.
    #[derive(Reconcile)]
    struct LooseIngredient {
        text: String,
        pantry: Option<String>,
    }
    #[derive(Reconcile)]
    struct LooseRecipe {
        schema_version: u32,
        title: String,
        servings: u32,
        effort: String,
        lead: Option<u32>,
        tags: std::collections::BTreeMap<String, String>,
        equipment: Vec<String>,
        ingredients: Vec<LooseIngredient>,
        source: Option<String>,
        status: String,
        body: Text,
    }

    let loose = LooseRecipe {
        schema_version: 1,
        title: "Loose ends".into(),
        servings: 4,
        effort: "weekday".into(),
        lead: None,
        tags: Default::default(),
        equipment: vec!["wok".into(), "not a slug!".into()],
        ingredients: vec![LooseIngredient {
            text: "200 g miso".into(),
            pantry: Some("a] b".into()),
        }],
        source: None,
        status: "x\n---\ntitle: forged".into(),
        body: Text::with_value("Cook loosely."),
    };
    let bytes = encode(99, &loose);

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    drop(Store::create(&root, &slug("home"), 2, jiff::Timestamp::UNIX_EPOCH).unwrap());
    inject(&root, &DocId::Recipe(slug("loose-ends")), &bytes);
    let mut store = Store::open(&root).unwrap();

    let doc: RecipeDoc = store.get(&DocId::Recipe(slug("loose-ends"))).unwrap();
    assert_eq!(doc.status, RecipeStatus::Draft, "an unknown status reads as draft");
    assert_eq!(doc.equipment, vec![slug("wok")], "a non-slug equipment entry is dropped");
    assert_eq!(doc.ingredients[0].pantry, None, "a non-slug pantry link is dropped");

    store.export("test: hostile recipe").unwrap();
    let page =
        std::fs::read_to_string(store.export_dir().join("recipes/loose-ends.md")).unwrap();
    assert!(!page.contains("forged"), "injected frontmatter reached the export:\n{page}");
    assert!(page.contains("status: draft"), "{page}");
}
