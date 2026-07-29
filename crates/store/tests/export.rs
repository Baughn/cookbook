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
        store.append_log(entry).unwrap();
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
}
