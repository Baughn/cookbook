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
