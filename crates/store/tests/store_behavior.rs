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
