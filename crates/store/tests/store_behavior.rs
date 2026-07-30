//! Store behavior: persistence across reopen, snapshot cadence, and the
//! git-committed export directory.

use mise_core::types::Slug;
use mise_store::pages::{PantryDoc, PantryItemDoc, StateDoc};
use mise_store::{DocId, Store, StoreError};

fn t0() -> jiff::Timestamp {
    jiff::Timestamp::UNIX_EPOCH
}

fn slug(s: &str) -> Slug {
    Slug::new(s).unwrap()
}

fn git_out(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn changes_survive_reopen_across_snapshot_cadence() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    let mut store = Store::create(&root, &slug("home"), 2, t0()).unwrap();

    // Enough modifications to cross the snapshot cadence (64).
    for i in 0..70u32 {
        store
            .modify::<PantryDoc>(&DocId::Pantry(slug("home")), "test: churn", t0(), |p| {
                p.items.insert(
                    format!("item{}", i % 7),
                    PantryItemDoc {
                        name: format!("item {i}"),
                        presence: "have".to_string(),
                        bought: None,
                        tier: None,
                        note: None,
                    },
                );
            })
            .unwrap();
    }
    let before: PantryDoc = store.get(&DocId::Pantry(slug("home"))).unwrap();
    drop(store);

    let store = Store::open(&root).unwrap();
    let after: PantryDoc = store.get(&DocId::Pantry(slug("home"))).unwrap();
    assert_eq!(after, before);
    assert_eq!(after.items["item6"].name, "item 69");

    // A snapshot row exists, and loading past it replayed correctly.
    let conn = rusqlite::Connection::open(root.join("mise.db")).unwrap();
    let snapshots: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM doc_snapshots WHERE doc_id = 'location/home/pantry'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(snapshots >= 1, "expected a snapshot after 70 changes");
}

#[test]
fn missing_and_duplicate_docs_are_errors() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    let mut store = Store::create(&root, &slug("home"), 2, t0()).unwrap();

    match store.get::<PantryDoc>(&DocId::Pantry(slug("cottage"))) {
        Err(StoreError::NotFound(id)) => assert_eq!(id, "location/cottage/pantry"),
        other => panic!("expected NotFound, got {other:?}"),
    }
    match store.create_doc(&DocId::Queue, &mise_store::pages::QueueDoc::empty(), "test", t0()) {
        Err(StoreError::Exists(_)) => {}
        other => panic!("expected Exists, got {other:?}"),
    }
    match Store::create(&root, &slug("home"), 2, t0()) {
        Err(StoreError::AlreadyInitialized(_)) => {}
        other => panic!("expected AlreadyInitialized, got {:?}", other.map(|_| ())),
    }
}

#[test]
fn export_commits_once_per_change_batch_and_prunes_stale_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    let mut store = Store::create(&root, &slug("home"), 2, t0()).unwrap();
    let export = store.export_dir();

    store.export("init: first export").unwrap();
    assert_eq!(git_out(&export, &["rev-list", "--count", "HEAD"]), "1");
    assert!(export.join("state.md").exists());
    assert!(export.join("locations/home/pantry.md").exists());

    // Exporting unchanged state commits nothing.
    store.export("noop").unwrap();
    assert_eq!(git_out(&export, &["rev-list", "--count", "HEAD"]), "1");

    // A change plus a stray file: the export regenerates and prunes.
    std::fs::create_dir_all(export.join("junk-dir")).unwrap();
    std::fs::write(export.join("junk-dir/stale.md"), "stale").unwrap();
    store
        .modify::<StateDoc>(&DocId::State, "test: bump headcount", t0(), |s| {
            s.locations.get_mut("home").unwrap().headcount = 3;
        })
        .unwrap();
    store.export("planning thread: headcount 3").unwrap();
    assert_eq!(git_out(&export, &["rev-list", "--count", "HEAD"]), "2");
    assert!(!export.join("junk-dir").exists(), "stale files are pruned");
    let last = git_out(&export, &["log", "-1", "--format=%s"]);
    assert_eq!(last, "planning thread: headcount 3");
    let state = std::fs::read_to_string(export.join("state.md")).unwrap();
    assert!(state.contains("| home | 3 |"), "{state}");
}

#[test]
fn every_doc_id_export_path_exists_in_the_render() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(&dir.path().join("corpus"), &slug("home"), 2, t0()).unwrap();
    store
        .create_doc(
            &DocId::Recipe(slug("mapo-tofu")),
            &mise_store::pages::RecipeDoc {
                schema_version: 1,
                title: "Mapo tofu".into(),
                servings: 4,
                effort: "weekday".into(),
                lead: None,
                tags: Default::default(),
                equipment: vec![],
                ingredients: vec![],
                status: "active".into(),
                body: "".into(),
            },
            "test",
        t0(),
        )
        .unwrap();
    store
        .create_doc(
            &DocId::Technique(slug("velveting")),
            &mise_store::pages::TechniqueDoc {
                schema_version: 1,
                title: "Velveting".into(),
                tags: Default::default(),
                body: "".into(),
            },
            "test",
        t0(),
        )
        .unwrap();

    let files = mise_store::render::render(&store.corpus().unwrap());
    let home = slug("home");
    for id in [
        DocId::State,
        DocId::Queue,
        DocId::Someday,
        DocId::Shopping,
        DocId::Steering,
        DocId::Facts,
        DocId::Pantry(home.clone()),
        DocId::Equipment(home.clone()),
        DocId::Shops(home.clone()),
        DocId::Fridge(home),
        DocId::Recipe(slug("mapo-tofu")),
        DocId::Technique(slug("velveting")),
    ] {
        assert!(
            files.contains_key(&id.export_path()),
            "export_path {} missing from render",
            id.export_path(),
        );
    }
}

// ---------------------------------------------------------- update_body --

fn recipe_with_body(root: &std::path::Path, body: &str) -> Store {
    let mut store = Store::create(root, &slug("home"), 2, t0()).unwrap();
    store
        .create_doc(
            &DocId::Recipe(slug("dish")),
            &mise_store::pages::RecipeDoc {
                schema_version: 1,
                title: "Dish".into(),
                servings: 2,
                effort: "weekday".into(),
                lead: None,
                tags: Default::default(),
                equipment: vec![],
                ingredients: vec![],
                status: "active".into(),
                body: body.into(),
            },
            "test",
        t0(),
        )
        .unwrap();
    store
}

#[test]
fn update_body_refuses_docs_without_prose() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(&dir.path().join("c"), &slug("home"), 2, t0()).unwrap();
    match store.update_body(&DocId::Queue, "x", "test", t0()) {
        Err(StoreError::Invalid(_)) => {}
        other => panic!("expected Invalid, got {:?}", other.map(|_| ())),
    }
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig {
        cases: 64, ..proptest::prelude::ProptestConfig::default()
    })]

    /// Regression for the non-ASCII body crash: autosurgeon's Text::update
    /// advanced splice positions in bytes against Automerge's char-indexed
    /// text. update_body must land any unicode body exactly, from any
    /// unicode predecessor, and survive a reload.
    #[test]
    fn update_body_round_trips_arbitrary_unicode(
        old in proptest::string::string_regex("[a-zæøå☃—×–\\n .#\\[\\]|;=\\\\]{0,60}").unwrap(),
        new in proptest::string::string_regex("[a-zæøå☃—×–\\n .#\\[\\]|;=\\\\]{0,60}").unwrap(),
    ) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("c");
        let mut store = recipe_with_body(&root, &old);
        store.update_body(&DocId::Recipe(slug("dish")), &new, "test: body", t0()).unwrap();

        let doc: mise_store::pages::RecipeDoc =
            store.get(&DocId::Recipe(slug("dish"))).unwrap();
        proptest::prop_assert_eq!(doc.body.as_str(), new.as_str());

        drop(store);
        let reopened = Store::open(&root).unwrap();
        let doc: mise_store::pages::RecipeDoc =
            reopened.get(&DocId::Recipe(slug("dish"))).unwrap();
        proptest::prop_assert_eq!(doc.body.as_str(), new.as_str());
    }
}

// ------------------------------------------------------ history / revert --

fn ts(secs: i64) -> jiff::Timestamp {
    jiff::Timestamp::from_second(secs).unwrap()
}

fn set_item(store: &mut Store, item: &str, presence: &str, at: jiff::Timestamp) {
    store
        .modify::<PantryDoc>(&DocId::Pantry(slug("home")), &format!("set {item}"), at, |p| {
            p.items.insert(
                item.to_string(),
                PantryItemDoc {
                    name: item.to_string(),
                    presence: presence.to_string(),
                    bought: None,
                    tier: None,
                    note: None,
                },
            );
        })
        .unwrap();
}

#[test]
fn history_carries_provenance_and_clock() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(&dir.path().join("c"), &slug("home"), 2, ts(100)).unwrap();
    set_item(&mut store, "miso", "have", ts(200));

    let history = store.history(&DocId::Pantry(slug("home"))).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].message, "init: empty corpus");
    assert_eq!(history[0].time, Some(ts(100)));
    assert_eq!(history[1].message, "set miso");
    assert_eq!(history[1].time, Some(ts(200)));
}

#[test]
fn revert_restores_a_structured_page_as_a_forward_change() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(&dir.path().join("c"), &slug("home"), 2, ts(1)).unwrap();
    let id = DocId::Pantry(slug("home"));
    set_item(&mut store, "miso", "have", ts(2));
    set_item(&mut store, "miso", "out", ts(3));
    set_item(&mut store, "wakame", "have", ts(4));

    let history = store.history(&id).unwrap();
    // Back to "miso: have", before the out-edit and wakame.
    store.revert(&id, &history[1].hash, "ui: revert", ts(5)).unwrap();

    let pantry: PantryDoc = store.get(&id).unwrap();
    assert_eq!(pantry.items.len(), 1);
    assert_eq!(pantry.items["miso"].presence, "have");
    let after = store.history(&id).unwrap();
    assert_eq!(after.len(), history.len() + 1, "revert is a new change, not erasure");
    assert_eq!(after.last().unwrap().message, "ui: revert");

    // Bad handles are refused.
    match store.revert(&id, "zz-not-a-hash", "ui: revert", ts(6)) {
        Err(StoreError::Invalid(_)) => {}
        other => panic!("expected Invalid, got {:?}", other.map(|_| ())),
    }
    let foreign = "0000000000000000000000000000000000000000000000000000000000000000";
    match store.revert(&id, foreign, "ui: revert", ts(7)) {
        Err(StoreError::NotFound(_)) => {}
        other => panic!("expected NotFound, got {:?}", other.map(|_| ())),
    }
}

#[test]
fn revert_restores_prose_pages_including_non_ascii_bodies() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("c");
    let mut store = recipe_with_body(&root, "Sauté the alliums — gently.");
    let id = DocId::Recipe(slug("dish"));
    store
        .modify::<mise_store::pages::RecipeDoc>(&id, "retitle", ts(2), |r| {
            r.title = "Dish, improved".into();
            r.servings = 6;
        })
        .unwrap();
    store.update_body(&id, "Blast the alliums.", "rewrite", ts(3)).unwrap();

    let history = store.history(&id).unwrap();
    store.revert(&id, &history[0].hash, "ui: revert", ts(4)).unwrap();

    let doc: mise_store::pages::RecipeDoc = store.get(&id).unwrap();
    assert_eq!(doc.title, "Dish");
    assert_eq!(doc.servings, 2);
    assert_eq!(doc.body.as_str(), "Sauté the alliums — gently.");
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig {
        cases: 32, ..proptest::prelude::ProptestConfig::default()
    })]

    /// Revert-to-any-point: after a random edit sequence, reverting to the
    /// k-th change reproduces exactly the state the page had then.
    #[test]
    fn revert_reaches_every_point_in_history(
        ops in proptest::collection::vec(
            (proptest::string::string_regex("[a-z]{1,6}").unwrap(), 0usize..3),
            1..8,
        ),
        pick in proptest::prelude::any::<proptest::sample::Index>(),
    ) {
        let dir = tempfile::tempdir().unwrap();
        let mut store =
            Store::create(&dir.path().join("c"), &slug("home"), 2, ts(0)).unwrap();
        let id = DocId::Pantry(slug("home"));

        let mut snapshots: Vec<PantryDoc> = vec![store.get(&id).unwrap()];
        for (i, (item, presence)) in ops.iter().enumerate() {
            // Suffix with the index so every op genuinely changes state —
            // a no-op modify writes no change and would desync the counts.
            set_item(
                &mut store,
                &format!("{item}{i}"),
                ["have", "low", "out"][*presence],
                ts(i as i64 + 1),
            );
            snapshots.push(store.get(&id).unwrap());
        }

        let history = store.history(&id).unwrap();
        proptest::prop_assert_eq!(history.len(), snapshots.len());
        let k = pick.index(history.len());
        store.revert(&id, &history[k].hash, "prop: revert", ts(1000)).unwrap();
        let now: PantryDoc = store.get(&id).unwrap();
        proptest::prop_assert_eq!(&now, &snapshots[k]);
    }
}

// ----------------------------------------------------- recipe status --

/// The first logged cook promotes a draft to active; the rule lives in the
/// store so no surface can forget it. Non-drafts are untouched — no doc
/// change at all.
#[test]
fn first_cook_promotes_a_draft_and_only_a_draft() {
    use std::collections::BTreeMap;

    use mise_core::types::{CookKind, LogEntry};
    use mise_store::pages::RecipeDoc;

    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(&dir.path().join("c"), &slug("home"), 2, ts(0)).unwrap();
    let recipe = |status: &str| RecipeDoc {
        schema_version: 1,
        title: "Dish".into(),
        servings: 2,
        effort: "weekday".into(),
        lead: None,
        tags: BTreeMap::new(),
        equipment: vec![],
        ingredients: vec![],
        status: status.into(),
        body: "Cook.".into(),
    };
    store
        .create_doc(&DocId::Recipe(slug("fresh-idea")), &recipe("draft"), "test: add", ts(1))
        .unwrap();
    store
        .create_doc(&DocId::Recipe(slug("old-flame")), &recipe("retired"), "test: add", ts(1))
        .unwrap();
    let cook = |s: &str| LogEntry {
        date: jiff::civil::Date::constant(2026, 7, 30),
        kind: CookKind::Meal,
        recipe: Some(slug(s)),
        title: "Dish".into(),
        location: "home".into(),
        servings: 2,
        verdict: "fine".into(),
        tags: BTreeMap::new(),
    };

    store.append_log(&cook("fresh-idea"), "test: first cook", ts(2)).unwrap();
    let promoted: RecipeDoc = store.get(&DocId::Recipe(slug("fresh-idea"))).unwrap();
    assert_eq!(promoted.status, "active");
    let history = store.history(&DocId::Recipe(slug("fresh-idea"))).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].message, "test: first cook");

    // A second cook, and cooks of non-draft recipes, change nothing.
    store.append_log(&cook("fresh-idea"), "test: second cook", ts(3)).unwrap();
    store.append_log(&cook("old-flame"), "test: cook retired", ts(3)).unwrap();
    assert_eq!(store.history(&DocId::Recipe(slug("fresh-idea"))).unwrap().len(), 2);
    let retired: RecipeDoc = store.get(&DocId::Recipe(slug("old-flame"))).unwrap();
    assert_eq!(retired.status, "retired");
    assert_eq!(store.history(&DocId::Recipe(slug("old-flame"))).unwrap().len(), 1);
}
