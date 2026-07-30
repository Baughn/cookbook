//! The JSON API over real HTTP: queue view with readiness, page reads,
//! history + revert round trip, thread transcripts, auth. No model — the
//! corpus is seeded directly.

use std::path::Path;

use jiff::civil::DateTime;
use mise_core::types::Slug;
use mise_server::{AppState, app};
use mise_store::pages::RecipeDoc;
use mise_store::threads::{Role, ThreadId};
use mise_store::{DocId, Store};
use serde_json::Value;

const TOKEN: &str = "test-token-0123456789abcdef";

fn slug(s: &str) -> Slug {
    Slug::new(s).unwrap()
}

fn ts(secs: i64) -> jiff::Timestamp {
    jiff::Timestamp::from_second(secs).unwrap()
}

fn seeded(dir: &Path) -> Store {
    let mut store = Store::create(&dir.join("server"), &slug("home"), 2, ts(1)).unwrap();
    store
        .create_doc(
            &DocId::Recipe(slug("mapo-tofu")),
            &RecipeDoc {
                schema_version: 1,
                title: "Mapo tofu".into(),
                servings: 4,
                effort: "weekday".into(),
                lead: None,
                tags: [("cuisine".to_string(), "sichuan".to_string())].into(),
                equipment: vec!["wok".into()],
                ingredients: vec![],
                source: None,
                status: "active".into(),
                body: "Fry the paste.".into(),
            },
            "seed",
            ts(2),
        )
        .unwrap();
    store
        .modify::<mise_store::pages::QueueDoc>(&DocId::Queue, "seed", ts(3), |q| {
            q.entries.insert(
                "mapo-tofu".into(),
                mise_store::pages::QueueEntryDoc {
                    dishes: vec![mise_store::pages::DishRefDoc {
                        recipe: Some("mapo-tofu".into()),
                        title: "Mapo tofu".into(),
                    }],
                    reason: Some("craving".into()),
                    added: "2026-07-29".into(),
                },
            );
        })
        .unwrap();
    store
        .append_thread_message(
            &ThreadId::Planning,
            Role::User,
            "plan the week",
            DateTime::constant(2026, 7, 29, 18, 0, 0, 0),
        )
        .unwrap();
    store.export("init").unwrap();
    store
}

async fn spawn(dir: &Path) -> String {
    let store = seeded(dir);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = AppState::new(store, TOKEN.to_string());
    tokio::spawn(async move {
        axum::serve(listener, app(state)).await.unwrap();
    });
    format!("http://{addr}")
}

async fn get_json(url: &str) -> Value {
    reqwest::Client::new()
        .get(url)
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn queue_view_carries_readiness_and_coverage() {
    let dir = tempfile::tempdir().unwrap();
    let url = spawn(dir.path()).await;

    let queue = get_json(&format!("{url}/api/queue")).await;
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
    let url = spawn(dir.path()).await;

    let pages = get_json(&format!("{url}/api/pages")).await;
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

    let page = get_json(&format!("{url}/api/page/recipes/mapo-tofu")).await;
    assert!(page["content"].as_str().unwrap().contains("# Mapo tofu"));

    let missing = reqwest::Client::new()
        .get(format!("{url}/api/page/recipes/nope"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
}

#[tokio::test]
async fn history_and_revert_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let url = spawn(dir.path()).await;
    let client = reqwest::Client::new();

    let history = get_json(&format!("{url}/api/history/queue")).await;
    let changes = history["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[1]["message"], "seed");
    assert_eq!(changes[1]["time"], "1970-01-01T00:00:03Z");

    // Revert the queue to its empty init state.
    let first_hash = changes[0]["hash"].as_str().unwrap();
    let resp = client
        .post(format!("{url}/api/revert"))
        .bearer_auth(TOKEN)
        .json(&serde_json::json!({"doc": "queue", "hash": first_hash}))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "{}", resp.status());

    let queue = get_json(&format!("{url}/api/queue")).await;
    assert_eq!(queue["entries"].as_array().unwrap().len(), 0);
    let history = get_json(&format!("{url}/api/history/queue")).await;
    let changes = history["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 3, "revert is a forward change");
    assert!(
        changes[2]["message"].as_str().unwrap().starts_with("ui: revert queue"),
        "{changes:?}",
    );

    let bad = client
        .post(format!("{url}/api/revert"))
        .bearer_auth(TOKEN)
        .json(&serde_json::json!({"doc": "queue", "hash": "zz"}))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);
}

#[tokio::test]
async fn threads_and_auth() {
    let dir = tempfile::tempdir().unwrap();
    let url = spawn(dir.path()).await;

    let thread = get_json(&format!("{url}/api/thread/planning")).await;
    assert_eq!(thread["messages"][0]["role"], "user");
    assert_eq!(thread["messages"][0]["content"], "plan the week");

    // Every /api route requires the token; ?token= works for browsers.
    let client = reqwest::Client::new();
    for path in ["api/queue", "api/pages", "api/page/queue", "api/history/queue", "api/thread/planning"] {
        let resp = client.get(format!("{url}/{path}")).send().await.unwrap();
        assert_eq!(resp.status(), 401, "{path}");
    }
    let resp = client
        .get(format!("{url}/api/queue?token={TOKEN}"))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
}

async fn post_json(url: &str, body: Value) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .post(url)
        .bearer_auth(TOKEN)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

/// The tap surface: each edit action is the corresponding assistant tool —
/// same validation, `ui:` provenance in the history, export refreshed.
#[tokio::test]
async fn edit_actions_mutate_through_the_tool_layer() {
    let dir = tempfile::tempdir().unwrap();
    let url = spawn(dir.path()).await;

    // A missing wok blocks mapo-tofu; add it through the tap.
    let (status, body) =
        post_json(&format!("{url}/api/edit/equipment-set"), serde_json::json!({"item": "wok"}))
            .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["ok"], true);

    let queue = get_json(&format!("{url}/api/queue")).await;
    let verdict = &queue["entries"][0]["dishes"][0]["verdict"]["kind"];
    assert_ne!(verdict, "missing-equipment", "the tap landed: {queue}");

    // Pantry set with a bad presence bounces off the tool's validation.
    let (status, body) = post_json(
        &format!("{url}/api/edit/pantry-set"),
        serde_json::json!({"item": "miso", "presence": "plenty"}),
    )
    .await;
    assert_eq!(status, 400);
    assert!(body["error"].as_str().unwrap().contains("presence"), "{body}");

    // Provenance in the doc's history says ui, not chat.
    let history = get_json(&format!("{url}/api/history/location/home/equipment")).await;
    let messages: Vec<&str> =
        history["changes"].as_array().unwrap().iter().map(|c| c["message"].as_str().unwrap()).collect();
    assert!(messages.iter().any(|m| m.starts_with("ui: equipment home: set wok")), "{messages:?}");

    // Unknown actions don't exist; no token, no edit.
    let (status, _) =
        post_json(&format!("{url}/api/edit/recipe-edit"), serde_json::json!({"slug": "x"})).await;
    assert_eq!(status, 404);
    let resp = reqwest::Client::new()
        .post(format!("{url}/api/edit/equipment-set"))
        .json(&serde_json::json!({"item": "wok"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

/// recipe-status flips only the status: nothing else in the payload makes
/// it through to the recipe.
#[tokio::test]
async fn recipe_status_narrows_to_the_status_field() {
    let dir = tempfile::tempdir().unwrap();
    let url = spawn(dir.path()).await;

    let (status, body) = post_json(
        &format!("{url}/api/edit/recipe-status"),
        serde_json::json!({
            "slug": "mapo-tofu",
            "status": "retired",
            "title": "Hijacked",
            "body": "Free-text through the back door."
        }),
    )
    .await;
    assert_eq!(status, 200, "{body}");

    let page = get_json(&format!("{url}/api/page/recipes/mapo-tofu")).await;
    let content = page["content"].as_str().unwrap();
    assert!(content.contains("status: retired"), "{content}");
    assert!(content.contains("# Mapo tofu"), "title untouched: {content}");
    assert!(!content.contains("Hijacked"));
    assert!(!content.contains("back door"));

    // And a bad status is the tool's error, verbatim.
    let (status, body) = post_json(
        &format!("{url}/api/edit/recipe-status"),
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
    let url = spawn(dir.path()).await;

    post_json(
        &format!("{url}/api/edit/pantry-set"),
        serde_json::json!({"item": "miso", "presence": "low", "tier": "shop"}),
    )
    .await;
    let loc = get_json(&format!("{url}/api/location")).await;
    assert_eq!(loc["location"], "home");
    assert_eq!(loc["view"]["pantry"]["miso"]["presence"], "low");
    assert_eq!(loc["view"]["pantry"]["miso"]["tier"], "shop");
    assert!(loc["view"]["tiers"].as_array().unwrap().iter().any(|t| t["id"] == "shop"));
}
