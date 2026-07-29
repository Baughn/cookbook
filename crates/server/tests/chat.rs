//! POST /chat end to end: real HTTP, real SSE, and a *scripted* model — a
//! fake Anthropic endpoint returning canned event streams. Charter: the
//! test suite never talks to a model, and this test doesn't; it exercises
//! everything up to the wire.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::response::IntoResponse;
use axum::routing::post;
use mise_core::types::Slug;
use mise_server::{AppState, ChatConfig, app};
use mise_store::pages::QueueDoc;
use mise_store::threads::{Role, ThreadId};
use mise_store::{DocId, Store};

const TOKEN: &str = "test-token-0123456789abcdef";

fn sse(events: &[(&str, &str)]) -> String {
    events
        .iter()
        .map(|(event, data)| format!("event: {event}\ndata: {data}\n\n"))
        .collect()
}

/// A fake api.anthropic.com: first call returns a queue_add tool use,
/// second call the closing text.
async fn spawn_fake_anthropic() -> String {
    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::new().route(
        "/v1/messages",
        post(move || {
            let calls = calls.clone();
            async move {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                let body = if n == 0 {
                    sse(&[
                        ("message_start", "{}"),
                        (
                            "content_block_start",
                            r#"{"content_block":{"type":"tool_use","id":"c1","name":"queue_add"}}"#,
                        ),
                        (
                            "content_block_delta",
                            r#"{"delta":{"type":"input_json_delta","partial_json":"{\"title\":\"Dal\",\"reason\":\"cheap\"}"}}"#,
                        ),
                        ("content_block_stop", "{}"),
                        ("message_delta", r#"{"delta":{"stop_reason":"tool_use"}}"#),
                        ("message_stop", "{}"),
                    ])
                } else {
                    sse(&[
                        ("message_start", "{}"),
                        (
                            "content_block_start",
                            r#"{"content_block":{"type":"text","text":""}}"#,
                        ),
                        (
                            "content_block_delta",
                            r#"{"delta":{"type":"text_delta","text":"Queued dal."}}"#,
                        ),
                        ("content_block_stop", "{}"),
                        ("message_delta", r#"{"delta":{"stop_reason":"end_turn"}}"#),
                        ("message_stop", "{}"),
                    ])
                };
                ([("content-type", "text/event-stream")], body).into_response()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}")
}

async fn spawn_mise(dir: &Path, chat: Option<ChatConfig>) -> (String, AppState) {
    let mut store = Store::create(&dir.join("server"), &Slug::new("home").unwrap(), 2, jiff::Timestamp::now()).unwrap();
    store.export("init: empty corpus").unwrap();
    let mut state = AppState::new(store, TOKEN.to_string());
    if let Some(config) = chat {
        state = state.with_chat(config);
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serve_state = state.clone();
    tokio::spawn(async move {
        axum::serve(listener, app(serve_state)).await.unwrap();
    });
    (format!("http://{addr}"), state)
}

#[tokio::test]
async fn chat_streams_tools_and_reply_and_persists() {
    let dir = tempfile::tempdir().unwrap();
    let fake = spawn_fake_anthropic().await;
    let config = ChatConfig {
        api_key: "test-key".into(),
        model: "claude-opus-5".into(),
        base_url: fake,
    };
    let (url, state) = spawn_mise(dir.path(), Some(config)).await;

    let body = reqwest::Client::new()
        .post(format!("{url}/chat"))
        .bearer_auth(TOKEN)
        .json(&serde_json::json!({"message": "plan something cheap"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();

    assert!(body.contains(r#"{"name":"queue_add"}"#), "tool event streamed: {body}");
    assert!(body.contains(r#"{"text":"Queued dal."}"#), "delta streamed: {body}");
    assert!(body.contains(r#""reply":"Queued dal.""#), "done event: {body}");

    // The exchange landed in the shared store: the edit, both thread turns
    // in order, and an export commit with thread provenance.
    let store = state.store.lock().await;
    let queue: QueueDoc = store.get(&DocId::Queue).unwrap();
    assert!(queue.entries.contains_key("dal"));
    let msgs = store.thread_messages(&ThreadId::Planning).unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].role, Role::User);
    assert_eq!(msgs[1].role, Role::Assistant);
    assert!(msgs[1].created > msgs[0].created);
    let transcript =
        std::fs::read_to_string(dir.path().join("server/export/threads/planning.md")).unwrap();
    assert!(transcript.contains("> Queued dal."), "{transcript}");
}

#[tokio::test]
async fn chat_without_model_is_503_and_bad_token_401() {
    let dir = tempfile::tempdir().unwrap();
    let (url, _state) = spawn_mise(dir.path(), None).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{url}/chat"))
        .bearer_auth(TOKEN)
        .json(&serde_json::json!({"message": "hi"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 503);

    let resp = client
        .post(format!("{url}/chat"))
        .bearer_auth("wrong-token-9876543210zyxwvu")
        .json(&serde_json::json!({"message": "hi"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn chat_about_a_missing_page_reports_an_error_event() {
    let dir = tempfile::tempdir().unwrap();
    let fake = spawn_fake_anthropic().await;
    let config = ChatConfig {
        api_key: "test-key".into(),
        model: "claude-opus-5".into(),
        base_url: fake,
    };
    let (url, _state) = spawn_mise(dir.path(), Some(config)).await;

    let body = reqwest::Client::new()
        .post(format!("{url}/chat"))
        .bearer_auth(TOKEN)
        .json(&serde_json::json!({"message": "hi", "page": "recipe/nope"}))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("event: error"), "{body}");
    assert!(body.contains("no page recipe/nope"), "{body}");
}
