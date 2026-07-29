//! Store behavior: persistence across reopen, snapshot cadence, and the
//! git-committed export directory.

use mise_core::types::Slug;
use mise_store::pages::{PantryDoc, PantryItemDoc, StateDoc};
use mise_store::{DocId, Store, StoreError};

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
    let mut store = Store::create(&root, &slug("home"), 2).unwrap();

    // Enough modifications to cross the snapshot cadence (64).
    for i in 0..70u32 {
        store
            .modify::<PantryDoc>(&DocId::Pantry(slug("home")), "test: churn", |p| {
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
    let mut store = Store::create(&root, &slug("home"), 2).unwrap();

    match store.get::<PantryDoc>(&DocId::Pantry(slug("cottage"))) {
        Err(StoreError::NotFound(id)) => assert_eq!(id, "location/cottage/pantry"),
        other => panic!("expected NotFound, got {other:?}"),
    }
    match store.create_doc(&DocId::Queue, &mise_store::pages::QueueDoc::empty(), "test") {
        Err(StoreError::Exists(_)) => {}
        other => panic!("expected Exists, got {other:?}"),
    }
    match Store::create(&root, &slug("home"), 2) {
        Err(StoreError::AlreadyInitialized(_)) => {}
        other => panic!("expected AlreadyInitialized, got {:?}", other.map(|_| ())),
    }
}

#[test]
fn export_commits_once_per_change_batch_and_prunes_stale_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    let mut store = Store::create(&root, &slug("home"), 2).unwrap();
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
        .modify::<StateDoc>(&DocId::State, "test: bump headcount", |s| {
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
    let mut store = Store::create(&dir.path().join("corpus"), &slug("home"), 2).unwrap();
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
                retired: false,
                body: "".into(),
            },
            "test",
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
    let mut store = Store::create(root, &slug("home"), 2).unwrap();
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
                retired: false,
                body: body.into(),
            },
            "test",
        )
        .unwrap();
    store
}

#[test]
fn update_body_refuses_docs_without_prose() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(&dir.path().join("c"), &slug("home"), 2).unwrap();
    match store.update_body(&DocId::Queue, "x", "test") {
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
        store.update_body(&DocId::Recipe(slug("dish")), &new, "test: body").unwrap();

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
