//! The JSON API over real HTTP: queue view with readiness, page reads,
//! history + revert round trip, thread transcripts, auth. No model — the
//! corpus is seeded directly.

mod support;

use support::{Server, seeded};

#[tokio::test]
async fn queue_view_carries_readiness_and_coverage() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::spawn(seeded(dir.path())).await;

    let queue = server.get_json("/api/queue").await;
    assert_eq!(queue["location"], "home");
    assert_eq!(queue["headcount"], 2);
    let entry = &queue["entries"][0];
    assert_eq!(entry["id"], "mapo-tofu");
    assert_eq!(entry["reason"], "craving");
    // The wok isn't recorded at home, so readiness says missing equipment.
    assert_eq!(entry["dishes"][0]["verdict"]["kind"], "missing-equipment");
    assert_eq!(entry["dishes"][0]["verdict"]["items"][0], "wok");
    assert_eq!(queue["coverage"]["dinners"], 0);
}

#[tokio::test]
async fn pages_and_page_reads() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::spawn(seeded(dir.path())).await;

    let pages = server.get_json("/api/pages").await;
    let list = pages["pages"].as_array().unwrap();
    let recipe = list
        .iter()
        .find(|p| p["path"] == "recipes/mapo-tofu.md")
        .expect("recipe listed");
    assert_eq!(recipe["doc"], "recipe/mapo-tofu");
    assert_eq!(recipe["title"], "Mapo tofu");
    assert_eq!(recipe["tags"]["cuisine"], "sichuan");
    let queue_page = list.iter().find(|p| p["path"] == "queue.md").unwrap();
    assert_eq!(queue_page["doc"], "queue");
    assert!(list.iter().any(|p| p["path"] == "threads/planning.md"));

    let page = server.get_json("/api/page/recipes/mapo-tofu").await;
    assert!(page["content"].as_str().unwrap().contains("# Mapo tofu"));

    assert_eq!(server.get("/api/page/recipes/nope").await.status(), 404);
}

#[tokio::test]
async fn history_and_revert_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::spawn(seeded(dir.path())).await;

    let history = server.get_json("/api/history/queue").await;
    let changes = history["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[1]["message"], "seed");
    assert_eq!(changes[1]["time"], "1970-01-01T00:00:03Z");

    // Revert the queue to its empty init state.
    let first_hash = changes[0]["hash"].as_str().unwrap();
    let (status, body) = server
        .post_json("/api/revert", serde_json::json!({"doc": "queue", "hash": first_hash}))
        .await;
    assert_eq!(status, 200, "{body}");

    let queue = server.get_json("/api/queue").await;
    assert_eq!(queue["entries"].as_array().unwrap().len(), 0);
    let history = server.get_json("/api/history/queue").await;
    let changes = history["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 3, "revert is a forward change");
    assert!(
        changes[2]["message"].as_str().unwrap().starts_with("ui: revert queue"),
        "{changes:?}",
    );

    let (status, _) = server
        .post_json("/api/revert", serde_json::json!({"doc": "queue", "hash": "zz"}))
        .await;
    assert_eq!(status, 400);

    // A hash whose 8th *byte* falls mid-character used to panic the
    // handler slicing it for the commit message — dropping the connection
    // where a 400 belonged.
    let (status, _) = server
        .post_json("/api/revert", serde_json::json!({"doc": "queue", "hash": "€€€"}))
        .await;
    assert_eq!(status, 400, "a multi-byte hash is the caller's error, not a panic");
}

// Auth coverage lives in tests/auth.rs as a route table; nothing here
// re-asserts it.
#[tokio::test]
async fn thread_transcripts_read_back() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::spawn(seeded(dir.path())).await;

    let thread = server.get_json("/api/thread/planning").await;
    assert_eq!(thread["messages"][0]["role"], "user");
    assert_eq!(thread["messages"][0]["content"], "plan the week");
}

/// The tap surface: each edit action is the corresponding assistant tool —
/// same validation, `ui:` provenance in the history, export refreshed.
#[tokio::test]
async fn edit_actions_mutate_through_the_tool_layer() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::spawn(seeded(dir.path())).await;

    // A missing wok blocks mapo-tofu; add it through the tap.
    let (status, body) =
        server.post_json("/api/edit/equipment-set", serde_json::json!({"item": "wok"})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["ok"], true);

    let queue = server.get_json("/api/queue").await;
    let verdict = &queue["entries"][0]["dishes"][0]["verdict"]["kind"];
    assert_ne!(verdict, "missing-equipment", "the tap landed: {queue}");

    // Pantry set with a bad presence bounces off the tool's validation.
    let (status, body) = server
        .post_json(
            "/api/edit/pantry-set",
            serde_json::json!({"item": "miso", "presence": "plenty"}),
        )
        .await;
    assert_eq!(status, 400);
    assert!(body["error"].as_str().unwrap().contains("presence"), "{body}");

    // Provenance in the doc's history says ui, not chat.
    let history = server.get_json("/api/history/location/home/equipment").await;
    let messages: Vec<&str> =
        history["changes"].as_array().unwrap().iter().map(|c| c["message"].as_str().unwrap()).collect();
    assert!(messages.iter().any(|m| m.starts_with("ui: equipment home: set wok")), "{messages:?}");

    // Unknown actions don't exist; no token, no edit.
    let (status, _) =
        server.post_json("/api/edit/recipe-edit", serde_json::json!({"slug": "x"})).await;
    assert_eq!(status, 404);
    let resp = server
        .post_anonymous("/api/edit/equipment-set", &serde_json::json!({"item": "wok"}))
        .await;
    assert_eq!(resp.status().as_u16(), 401);
}

/// recipe-status flips only the status: nothing else in the payload makes
/// it through to the recipe.
#[tokio::test]
async fn recipe_status_narrows_to_the_status_field() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::spawn(seeded(dir.path())).await;

    let (status, body) = server
        .post_json(
            "/api/edit/recipe-status",
            serde_json::json!({
                "slug": "mapo-tofu",
                "status": "retired",
                "title": "Hijacked",
                "body": "Free-text through the back door."
            }),
        )
        .await;
    assert_eq!(status, 200, "{body}");

    let page = server.get_json("/api/page/recipes/mapo-tofu").await;
    let content = page["content"].as_str().unwrap();
    assert!(content.contains("status: retired"), "{content}");
    assert!(content.contains("# Mapo tofu"), "title untouched: {content}");
    assert!(!content.contains("Hijacked"));
    assert!(!content.contains("back door"));

    // And a bad status is the tool's error, verbatim.
    let (status, body) = server
        .post_json(
            "/api/edit/recipe-status",
            serde_json::json!({"slug": "mapo-tofu", "status": "paused"}),
        )
        .await;
    assert_eq!(status, 400);
    assert!(body["error"].as_str().unwrap().contains("unknown recipe status"), "{body}");
}

/// The structured location view feeds the item editors — the exported
/// markdown is never parsed by the app.
#[tokio::test]
async fn location_view_is_structured() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::spawn(seeded(dir.path())).await;

    server
        .post_json(
            "/api/edit/pantry-set",
            serde_json::json!({"item": "miso", "presence": "low", "tier": "shop"}),
        )
        .await;
    let loc = server.get_json("/api/location").await;
    assert_eq!(loc["location"], "home");
    assert_eq!(loc["view"]["pantry"]["miso"]["presence"], "low");
    assert_eq!(loc["view"]["pantry"]["miso"]["tier"], "shop");
    assert!(loc["view"]["tiers"].as_array().unwrap().iter().any(|t| t["id"] == "shop"));
}

/// One dangling recipe reference in the queue doc degrades one row; the
/// home screen never 404s over it.
#[tokio::test]
async fn a_dangling_recipe_reference_does_not_404_the_queue() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = seeded(dir.path());
    store
        .modify::<mise_store::pages::QueueDoc>(
            &mise_store::DocId::Queue,
            "sync skew",
            jiff::Timestamp::from_second(9).unwrap(),
            |q| {
                q.entries.insert(
                    "ghost".into(),
                    mise_store::pages::QueueEntryDoc {
                        dishes: vec![mise_store::pages::DishRefDoc {
                            recipe: Some("ghost-dish".into()),
                            title: "Ghost dish".into(),
                        }],
                        reason: None,
                        added: "2026-07-28".into(),
                    },
                );
            },
        )
        .unwrap();
    let server = Server::spawn(store).await;

    let queue = server.get_json("/api/queue").await;
    let entries = queue["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "{queue}");
    let ghost = entries.iter().find(|e| e["id"] == "ghost").unwrap();
    assert_eq!(ghost["dishes"][0]["verdict"]["kind"], "recipe-missing", "{ghost}");
}
