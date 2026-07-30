//! Context assembly: deterministic, cache-layered, page-aware.

use jiff::civil::DateTime;
use mise_assistant::context::{assemble, provenance};
use mise_assistant::seam::ChatRole;
use mise_core::types::Slug;
use mise_store::threads::{Role, ThreadId};
use mise_store::{DocId, Store};

fn now() -> DateTime {
    DateTime::constant(2026, 7, 29, 18, 30, 0, 0)
}

fn fresh() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(&dir.path().join("corpus"), &Slug::new("home").unwrap(), 2, jiff::Timestamp::UNIX_EPOCH).unwrap();
    (dir, store)
}

#[test]
fn assembly_is_deterministic_and_layered_for_caching() {
    let (_dir, mut store) = fresh();
    store
        .modify::<mise_store::pages::SteeringDoc>(&DocId::Steering, "test", jiff::Timestamp::UNIX_EPOCH, |s| {
            s.entries.insert("yeast".into(), "beyond sourdough".into());
        })
        .unwrap();

    let (sys1, hist1) = assemble(&store, &ThreadId::Planning, now()).unwrap();
    let (sys2, hist2) = assemble(&store, &ThreadId::Planning, now()).unwrap();
    assert_eq!(sys1, sys2);
    assert_eq!(hist1, hist2);

    assert!(sys1.contains("beyond sourdough"), "steering is ambient context");
    assert!(sys1.contains("| home | 2 |"), "state is ambient context");
    assert!(sys1.contains("planning thread"), "planning flavor");
    assert!(sys1.ends_with("The current date and time: 2026-07-29 18:30."), "clock last");

    // A different clock changes nothing before the final line — the whole
    // prefix stays prompt-cacheable across turns.
    let (sys3, _) = assemble(&store, &ThreadId::Planning, now().saturating_add(jiff::SignedDuration::from_hours(26))).unwrap();
    let cut = sys1.rfind("\nThe current date and time:").unwrap();
    assert_eq!(&sys3[..cut], &sys1[..cut]);
    assert_ne!(sys3, sys1);
}

#[test]
fn page_threads_carry_their_page() {
    let (_dir, mut store) = fresh();
    let slug = Slug::new("mapo-tofu").unwrap();
    store
        .create_doc(
            &DocId::Recipe(slug.clone()),
            &mise_store::pages::RecipeDoc {
                schema_version: 1,
                title: "Mapo tofu".into(),
                servings: 4,
                effort: "weekday".into(),
                lead: None,
                tags: Default::default(),
                equipment: vec![],
                ingredients: vec![],
                source: None,
                status: "active".into(),
                body: "Fry the paste.".into(),
            },
            "test",
            jiff::Timestamp::UNIX_EPOCH,
        )
        .unwrap();

    let thread = ThreadId::Page(DocId::Recipe(slug));
    let (sys, _) = assemble(&store, &thread, now()).unwrap();
    assert!(sys.contains("## This page — recipes/mapo-tofu.md"), "{sys}");
    assert!(sys.contains("Fry the paste."), "page body included");
    assert!(sys.contains("debrief"), "page flavor");
    assert!(!sys.contains("planning thread"), "not the planning flavor");

    assert_eq!(provenance(&thread), "thread recipe/mapo-tofu");
    assert_eq!(provenance(&ThreadId::Planning), "planning thread");
}

#[test]
fn history_maps_thread_messages_in_order() {
    let (_dir, mut store) = fresh();
    let t = ThreadId::Planning;
    store.append_thread_message(&t, Role::User, "plan the week", now()).unwrap();
    store
        .append_thread_message(
            &t,
            Role::Assistant,
            "three dishes: …",
            now().saturating_add(jiff::SignedDuration::from_secs(30)),
        )
        .unwrap();

    let (_, history) = assemble(&store, &t, now()).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].role, ChatRole::User);
    assert_eq!(history[1].role, ChatRole::Assistant);
}
