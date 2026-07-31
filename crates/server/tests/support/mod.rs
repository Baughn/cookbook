//! Shared fixtures for the server's integration tests: one corpus builder,
//! one way to start a server, one way to make an authorized request.
//!
//! Three divergent copies of this used to live in `api.rs`, `chat.rs` and
//! `sync_ws.rs`, each with its own token, its own seed and its own idea of
//! what a request looks like — so a test asserting on auth in one file said
//! nothing about the other two. The remediation campaign adds regression
//! tests across all three; they should not each invent a server.
//!
//! `cli/tests/remote.rs` deliberately stays separate: it is a different
//! crate, and it drives the server as a black box over the network.

// Each test binary compiles the whole module but uses a different slice.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::Path;

use jiff::civil::DateTime;
use mise_core::types::Slug;
use mise_server::{AppState, ChatConfig, app};
use mise_store::pages::{DishRefDoc, QueueDoc, QueueEntryDoc, RecipeDoc};
use mise_store::threads::{Role, ThreadId};
use mise_store::{DocId, Store};
use serde_json::Value;

/// The bearer token every fixture server is started with.
pub const TOKEN: &str = "test-token-0123456789abcdef";

/// A token of the right shape that is not [`TOKEN`].
pub const WRONG_TOKEN: &str = "wrong-token-9876543210zyxwvu";

pub fn slug(s: &str) -> Slug {
    Slug::new(s).expect("fixture slugs are valid")
}

pub fn ts(secs: i64) -> jiff::Timestamp {
    jiff::Timestamp::from_second(secs).expect("fixture timestamps are in range")
}

/// A fresh corpus at `<dir>/server` with nothing in it but the initial
/// export — the starting point for tests that create their own state.
pub fn empty(dir: &Path) -> Store {
    let mut store = Store::create(&dir.join("server"), &slug("home"), 2, ts(0)).unwrap();
    store.export("init: empty corpus").unwrap();
    store
}

/// The corpus the read-side tests are written against: one recipe that
/// needs a wok, one queue entry naming it, one thread message. Home records
/// no equipment, so mapo-tofu is deliberately *not* ready — readiness having
/// something to say is the point of the fixture.
pub fn seeded(dir: &Path) -> Store {
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
        .modify::<QueueDoc>(&DocId::Queue, "seed", ts(3), |q| {
            q.entries.insert(
                "mapo-tofu".into(),
                QueueEntryDoc {
                    dishes: vec![DishRefDoc {
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

/// A server listening on a loopback port, and the state it shares with the
/// test — `state.store` is the same store the handlers mutate, so a test can
/// assert on the corpus without going back over HTTP.
pub struct Server {
    pub addr: SocketAddr,
    pub state: AppState,
    pub client: reqwest::Client,
}

impl Server {
    /// Start a server with no model configured. `/chat` answers 503.
    pub async fn spawn(store: Store) -> Server {
        Server::start(store, None).await
    }

    /// Start a server pointed at a scripted model endpoint.
    pub async fn spawn_with_chat(store: Store, chat: ChatConfig) -> Server {
        Server::start(store, Some(chat)).await
    }

    async fn start(store: Store, chat: Option<ChatConfig>) -> Server {
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
        Server { addr, state, client: reqwest::Client::new() }
    }

    /// `http://<addr><path>` — `path` starts with a slash.
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    /// `ws://<addr><path>` — same server, WebSocket scheme.
    pub fn ws_url(&self, path: &str) -> String {
        format!("ws://{}{path}", self.addr)
    }

    /// GET with the bearer token, whatever the status.
    pub async fn get(&self, path: &str) -> reqwest::Response {
        self.client.get(self.url(path)).bearer_auth(TOKEN).send().await.unwrap()
    }

    /// GET with no credentials at all.
    pub async fn get_anonymous(&self, path: &str) -> reqwest::Response {
        self.client.get(self.url(path)).send().await.unwrap()
    }

    /// GET expecting success, decoded as JSON.
    pub async fn get_json(&self, path: &str) -> Value {
        self.get(path).await.error_for_status().unwrap().json().await.unwrap()
    }

    /// POST with the bearer token, whatever the status.
    pub async fn post(&self, path: &str, body: &Value) -> reqwest::Response {
        self.client.post(self.url(path)).bearer_auth(TOKEN).json(body).send().await.unwrap()
    }

    /// POST with no credentials at all.
    pub async fn post_anonymous(&self, path: &str, body: &Value) -> reqwest::Response {
        self.client.post(self.url(path)).json(body).send().await.unwrap()
    }

    /// POST returning status alongside the body, since half these tests are
    /// about the status and the error body together. A non-JSON body reads
    /// as `Value::Null` rather than panicking — an error page is a result.
    pub async fn post_json(&self, path: &str, body: Value) -> (u16, Value) {
        let resp = self.post(path, &body).await;
        let status = resp.status().as_u16();
        (status, resp.json().await.unwrap_or(Value::Null))
    }

    /// POST reading the response as text — the SSE tests want the raw stream.
    pub async fn post_text(&self, path: &str, body: Value) -> String {
        self.post(path, &body).await.text().await.unwrap()
    }
}
