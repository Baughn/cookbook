//! The export never lies: property tests for export determinism (same doc
//! state → byte-identical files, including across a full save/reload cycle)
//! and completeness (everything in the store is legible in the export,
//! verified by the test-only parser).

mod support;

use std::path::Path;

use mise_core::types::Slug;
use mise_store::pages::CorpusState;
use mise_store::render::render;
use mise_store::{DocId, Store};
use proptest::prelude::*;

fn t0() -> jiff::Timestamp {
    jiff::Timestamp::UNIX_EPOCH
}

fn store_with(c: &CorpusState, root: &Path) -> Store {
    let mut store = Store::create_bare(root).unwrap();
    let p = "test: seed corpus";
    store.create_doc(&DocId::State, &c.state, p, t0()).unwrap();
    store.create_doc(&DocId::Queue, &c.queue, p, t0()).unwrap();
    store.create_doc(&DocId::Someday, &c.someday, p, t0()).unwrap();
    store.create_doc(&DocId::Shopping, &c.shopping, p, t0()).unwrap();
    store.create_doc(&DocId::Steering, &c.steering, p, t0()).unwrap();
    store.create_doc(&DocId::Facts, &c.facts, p, t0()).unwrap();
    for (name, docs) in &c.locations {
        let loc = Slug::new(name.as_str()).unwrap();
        store.create_doc(&DocId::Pantry(loc.clone()), &docs.pantry, p, t0()).unwrap();
        store.create_doc(&DocId::Equipment(loc.clone()), &docs.equipment, p, t0()).unwrap();
        store.create_doc(&DocId::Shops(loc.clone()), &docs.shops, p, t0()).unwrap();
        store.create_doc(&DocId::Fridge(loc.clone()), &docs.fridge, p, t0()).unwrap();
    }
    for (slug, recipe) in &c.recipes {
        store
            .create_doc(&DocId::Recipe(Slug::new(slug.as_str()).unwrap()), recipe, p, t0())
            .unwrap();
    }
    for (slug, technique) in &c.techniques {
        store
            .create_doc(&DocId::Technique(Slug::new(slug.as_str()).unwrap()), technique, p, t0())
            .unwrap();
    }
    for entry in &c.log {
        store.append_log(entry, p, t0()).unwrap();
    }
    for messages in c.threads.values() {
        for m in messages {
            store
                .append_thread_message(&m.thread, m.role, &m.content, m.created)
                .unwrap();
        }
    }
    store
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

    #[test]
    fn export_is_deterministic_and_complete(c in support::arb_corpus()) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        let store = store_with(&c, &root);
        let c1 = store.corpus().unwrap();
        let files1 = render(&c1);

        // Completeness: export → parse → structural compare against the
        // store state. Nothing may exist only in SQLite.
        prop_assert_eq!(support::parse_corpus(&files1), c1.clone());

        // Determinism across a full save/reload cycle: reopening the store
        // and re-rendering yields byte-identical files.
        drop(store);
        let reopened = Store::open(&root).unwrap();
        let c2 = reopened.corpus().unwrap();
        prop_assert_eq!(&c2, &c1);
        prop_assert_eq!(render(&c2), files1);
    }

    /// The single-page renderer the assistant's context assembly uses
    /// agrees byte-for-byte with the full export: same doc, same bytes.
    /// Without this, what the model reads could drift from what the
    /// export (and the user) sees.
    #[test]
    fn a_page_rendered_alone_matches_its_export(c in support::arb_corpus()) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        let store = store_with(&c, &root);
        let corpus = store.corpus().unwrap();
        let files = render(&corpus);

        let mut ids = vec![
            DocId::State,
            DocId::Queue,
            DocId::Someday,
            DocId::Shopping,
            DocId::Steering,
            DocId::Facts,
        ];
        for name in corpus.locations.keys() {
            let loc = Slug::new(name.as_str()).unwrap();
            ids.push(DocId::Pantry(loc.clone()));
            ids.push(DocId::Equipment(loc.clone()));
            ids.push(DocId::Shops(loc.clone()));
            ids.push(DocId::Fridge(loc));
        }
        for slug in corpus.recipes.keys() {
            ids.push(DocId::Recipe(Slug::new(slug.as_str()).unwrap()));
        }
        for slug in corpus.techniques.keys() {
            ids.push(DocId::Technique(Slug::new(slug.as_str()).unwrap()));
        }
        for id in ids {
            prop_assert_eq!(
                &store.render_page(&id).unwrap(),
                files.get(&id.export_path()).unwrap_or_else(|| panic!("no export for {id:?}")),
                "render_page disagrees with the export for {:?}", id
            );
        }

        // Audit #34: the path-addressed narrow read must cover *every* page
        // in the export — docs, log months and thread transcripts — byte
        // for byte, so `read_page` never needs the whole-corpus render. And
        // a path the export does not contain is a miss, not an empty page.
        for (path, content) in &files {
            let got = store.render_export_page(path).unwrap();
            prop_assert_eq!(
                got.as_ref(),
                Some(content),
                "render_export_page disagrees with the export for {:?}", path
            );
        }
        prop_assert_eq!(store.render_export_page("recipes/no-such-dish.md").unwrap(), None);
        prop_assert_eq!(store.render_export_page("log/1900-01.md").unwrap(), None);
        prop_assert_eq!(store.render_export_page("threads/planning.md").unwrap().is_none(),
            !files.contains_key("threads/planning.md"));
        prop_assert_eq!(store.render_export_page("not-a-page.md").unwrap(), None);
    }
}

/// "The export is derived — deletable and regenerable at any time." Deleting
/// it must therefore be recoverable by the next export, not a permanent
/// failure: every mutation ends in export(), the failure lands *after* the
/// SQLite write committed, and the natural retry then duplicates log rows.
#[test]
fn the_export_regenerates_itself_after_deletion() {
    use std::process::Command;
    let git_commits = |dir: &Path| -> usize {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["rev-list", "--count", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0)
    };

    let dir = tempfile::tempdir().unwrap();
    let mut store =
        Store::create(dir.path(), &Slug::new("home").unwrap(), 2, t0()).unwrap();
    store.export("first").unwrap();

    // The whole directory goes away — a cloud-sync mishap, a curious rm.
    std::fs::remove_dir_all(store.export_dir()).unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store.export("after deletion").unwrap();
    assert!(store.export_dir().join("state.md").exists(), "the export came back");
    assert_eq!(git_commits(&store.export_dir()), 1, "a fresh repo with the regenerated tree");

    // Only .git goes away — the tree survives but the repo is gone.
    std::fs::remove_dir_all(store.export_dir().join(".git")).unwrap();
    store.export("after .git deletion").unwrap();
    assert_eq!(git_commits(&store.export_dir()), 1, "re-initialized and committed");
}
