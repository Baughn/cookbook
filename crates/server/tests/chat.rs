//! POST /chat end to end: real HTTP, real SSE, and a *scripted* model — a
//! fake Anthropic endpoint returning canned event streams. Charter: the
//! test suite never talks to a model, and this test doesn't; it exercises
//! everything up to the wire.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::response::IntoResponse;
use axum::routing::post;
use mise_server::ChatConfig;
use mise_store::pages::QueueDoc;
use mise_store::threads::{Role, ThreadId};
use mise_store::DocId;
use support::{Server, WRONG_TOKEN, empty};

fn sse(events: &[(&str, &str)]) -> String {
    events
        .iter()
        .map(|(event, data)| format!("event: {event}\ndata: {data}\n\n"))
        .collect()
}

/// Serve a scripted `/v1/messages`: call *n* answers with body *n*, and the
/// last body repeats for any further calls.
async fn spawn_fake_anthropic(bodies: Vec<String>) -> ChatConfig {
    let calls = Arc::new(AtomicUsize::new(0));
    let bodies = Arc::new(bodies);
    let router = Router::new().route(
        "/v1/messages",
        post(move || {
            let calls = calls.clone();
            let bodies = bodies.clone();
            async move {
                let n = calls.fetch_add(1, Ordering::SeqCst).min(bodies.len() - 1);
                ([("content-type", "text/event-stream")], bodies[n].clone()).into_response()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    ChatConfig {
        api_key: "test-key".into(),
        model: "claude-opus-5".into(),
        base_url: format!("http://{addr}"),
    }
}

/// One streamed tool call, as the model would emit it.
fn tool_use(name: &str, input: &serde_json::Value) -> String {
    let delta = serde_json::json!({
        "delta": {"type": "input_json_delta", "partial_json": input.to_string()},
    });
    sse(&[
        ("message_start", "{}"),
        (
            "content_block_start",
            &format!(r#"{{"content_block":{{"type":"tool_use","id":"c1","name":"{name}"}}}}"#),
        ),
        ("content_block_delta", &delta.to_string()),
        ("content_block_stop", "{}"),
        ("message_delta", r#"{"delta":{"stop_reason":"tool_use"}}"#),
        ("message_stop", "{}"),
    ])
}

/// One streamed text reply, ending the turn.
fn text_reply(text: &str) -> String {
    let delta = serde_json::json!({"delta": {"type": "text_delta", "text": text}});
    sse(&[
        ("message_start", "{}"),
        ("content_block_start", r#"{"content_block":{"type":"text","text":""}}"#),
        ("content_block_delta", &delta.to_string()),
        ("content_block_stop", "{}"),
        ("message_delta", r#"{"delta":{"stop_reason":"end_turn"}}"#),
        ("message_stop", "{}"),
    ])
}

#[tokio::test]
async fn chat_streams_tools_and_reply_and_persists() {
    let dir = tempfile::tempdir().unwrap();
    let chat = spawn_fake_anthropic(vec![
        tool_use("queue_add", &serde_json::json!({"title": "Dal", "reason": "cheap"})),
        text_reply("Queued dal."),
    ])
    .await;
    let server = Server::spawn_with_chat(empty(dir.path()), chat).await;

    let body = server.post_text("/chat", serde_json::json!({"message": "plan something cheap"})).await;

    assert!(body.contains(r#"{"name":"queue_add"}"#), "tool event streamed: {body}");
    assert!(body.contains(r#"{"text":"Queued dal."}"#), "delta streamed: {body}");
    assert!(body.contains(r#""reply":"Queued dal.""#), "done event: {body}");

    // The exchange landed in the shared store: the edit, both thread turns
    // in order, and an export commit with thread provenance.
    let store = server.state.store.lock().await;
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
async fn recon_proposal_outlives_the_exchange_until_every_line_holds() {
    let dir = tempfile::tempdir().unwrap();
    let chat = spawn_fake_anthropic(vec![
        tool_use(
            "propose_pantry_diff",
            &serde_json::json!({
                "location": "home",
                "lines": [
                    {"item": "miso", "presence": "out", "reason": "no jar visible"},
                    {"item": "rice", "presence": "have", "name": "Rice", "reason": "big bag"},
                ],
            }),
        ),
        text_reply("Tap what fits."),
    ])
    .await;
    let server = Server::spawn_with_chat(empty(dir.path()), chat).await;
    let thread = || server.get_json("/api/thread/location/home/pantry");

    // Before any recon, the thread carries no proposal.
    assert_eq!(thread().await["proposal"], serde_json::Value::Null);

    let body = server
        .post_text(
            "/chat",
            serde_json::json!({"message": "shelf photo", "page": "location/home/pantry"}),
        )
        .await;
    assert!(body.contains("event: proposal"), "proposal streamed: {body}");

    // The exchange is over; the proposal is still live on the thread, its
    // applied-state read off the pantry itself (nothing applied yet).
    let proposal = thread().await["proposal"].clone();
    let lines = proposal["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["item"], "miso");
    assert_eq!(lines[0]["current"], serde_json::Value::Null);

    // Apply one line as the UI would; the annotation follows the pantry.
    async fn apply(server: &Server, item: &str, presence: &str) {
        let (status, body) = server
            .post_json(
                "/api/edit/pantry-set",
                serde_json::json!({"item": item, "presence": presence, "location": "home"}),
            )
            .await;
        assert_eq!(status, 200, "{body}");
    }
    apply(&server, "miso", "out").await;
    let proposal = thread().await["proposal"].clone();
    assert_eq!(proposal["lines"][0]["current"], "out");
    assert_eq!(proposal["lines"][1]["current"], serde_json::Value::Null);

    // Once every line holds, the proposal is completed and gone.
    apply(&server, "rice", "have").await;
    assert_eq!(thread().await["proposal"], serde_json::Value::Null);
}

#[tokio::test]
async fn chat_without_model_is_503_and_bad_token_401() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::spawn(empty(dir.path())).await;

    let (status, _) = server.post_json("/chat", serde_json::json!({"message": "hi"})).await;
    assert_eq!(status, 503);

    let resp = server
        .client
        .post(server.url("/chat"))
        .bearer_auth(WRONG_TOKEN)
        .json(&serde_json::json!({"message": "hi"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn chat_about_a_missing_page_reports_an_error_event() {
    let dir = tempfile::tempdir().unwrap();
    let chat = spawn_fake_anthropic(vec![text_reply("Queued dal.")]).await;
    let server = Server::spawn_with_chat(empty(dir.path()), chat).await;

    let body = server
        .post_text("/chat", serde_json::json!({"message": "hi", "page": "recipe/nope"}))
        .await;
    assert!(body.contains("event: error"), "{body}");
    assert!(body.contains("no page recipe/nope"), "{body}");
}
