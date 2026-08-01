//! The Mise server: a thin, always-on replica. It holds the corpus, speaks
//! the sync protocol over WebSocket to any client, hosts the assistant
//! over streaming HTTP, and re-exports after every session. Caddy
//! terminates TLS in front of it; auth is a single static bearer token —
//! one user, no accounts, no sessions to expire.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::ws::{Message as WsMessage, WebSocket};
use axum::extract::{Json, Query, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::StreamExt;
use mise_store::Store;
use mise_store::sync::{Peer, WireMsg};
use tokio::sync::Mutex;
use tracing::{info, warn};

mod api;
mod chat;

/// Everything the assistant endpoint needs to reach the model. Absent when
/// the deployment is sync-only (no Anthropic key configured).
#[derive(Clone)]
pub struct ChatConfig {
    pub api_key: String,
    pub model: String,
    /// Overridable for tests and proxies.
    pub base_url: String,
}

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Mutex<Store>>,
    pub token: Arc<String>,
    pub chat: Option<Arc<ChatConfig>>,
    /// Built web app to serve at `/`; sync/API-only without it.
    pub static_dir: Option<Arc<std::path::PathBuf>>,
    /// The latest recon proposal per thread — live until completed or
    /// superseded, so its Apply taps outlast the exchange (and a phone
    /// tab reload). In memory only: it is ephemeral working state like
    /// the photos it came from, never store state the export would owe.
    pub proposals: Arc<Mutex<HashMap<String, mise_assistant::recon::Proposal>>>,
}

impl AppState {
    pub fn new(store: Store, token: String) -> AppState {
        AppState {
            store: Arc::new(Mutex::new(store)),
            token: Arc::new(token),
            chat: None,
            static_dir: None,
            proposals: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn with_chat(mut self, config: ChatConfig) -> AppState {
        self.chat = Some(Arc::new(config));
        self
    }

    pub fn with_static_dir(mut self, dir: std::path::PathBuf) -> AppState {
        self.static_dir = Some(Arc::new(dir));
        self
    }
}

pub fn app(state: AppState) -> Router {
    // Every route in this sub-router sits behind the auth layer, which
    // decides on the request head — before any body is buffered. A route
    // added here is authed by default.
    let authed = Router::new()
        // A downscaled photo in base64 blows straight past axum's 2 MB
        // default; the Photo validator enforces the real ceiling.
        .route(
            "/chat",
            post(chat_endpoint).layer(axum::extract::DefaultBodyLimit::max(12 * 1024 * 1024)),
        )
        .route("/api/queue", get(api::queue))
        .route("/api/pages", get(api::pages))
        .route("/api/page/{*path}", get(api::page))
        .route("/api/history/{*doc}", get(api::history))
        .route("/api/revert", post(api::revert))
        .route("/api/location", get(api::location))
        .route("/api/edit/{action}", post(api::edit))
        .route("/api/thread/{*thread}", get(api::thread))
        .layer(axum::middleware::from_fn_with_state(state.clone(), require_auth));
    let mut router = Router::new()
        .route("/health", get(|| async { "ok" }))
        // /sync authenticates in its own handler: browsers cannot set
        // headers on a WebSocket handshake, so it alone accepts ?token=.
        .route("/sync", get(ws_sync))
        .merge(authed);
    if let Some(dir) = &state.static_dir {
        // The SvelteKit build is a static SPA: unknown paths fall back to
        // index.html and the app routes client-side. The fallback hangs on
        // the outer router — the SPA must render its token prompt before it
        // has a token to send.
        let serve = tower_http::services::ServeDir::new(dir.as_ref())
            .fallback(tower_http::services::ServeFile::new(dir.join("index.html")));
        router = router.fallback_service(serve);
    }
    router.with_state(state)
}

#[derive(serde::Deserialize)]
pub struct ChatRequest {
    pub message: String,
    /// Page-thread doc id (`recipe/mapo-tofu`); omitted = planning thread.
    pub page: Option<String>,
    /// Photos riding this exchange (pantry recon — a shelf rarely fits one
    /// frame). Transient: attached to the outgoing model turn, never
    /// stored.
    #[serde(default)]
    pub images: Vec<ChatImage>,
}

#[derive(serde::Deserialize)]
pub struct ChatImage {
    pub media_type: String,
    /// Base64 payload; the client downscales before upload.
    pub data: String,
}

/// POST /chat: one conversational exchange, streamed back as SSE (`delta`,
/// `tool`, `done`, `error` events). The store lock is held only around
/// store work — never across model calls — so sync sessions keep flowing
/// while the model thinks.
async fn chat_endpoint(State(state): State<AppState>, Json(request): Json<ChatRequest>) -> Response {
    let Some(config) = state.chat.clone() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "no model configured on this server")
            .into_response();
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    tokio::spawn(chat::drive(state, config, request, tx));
    let stream = futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx))
        .map(Ok::<_, std::convert::Infallible>);
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

/// Constant-time-ish comparison; the length is not treated as secret.
fn token_matches(provided: &str, expected: &str) -> bool {
    let (a, b) = (provided.as_bytes(), expected.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Bearer token from the Authorization header — the only place HTTP routes
/// accept one. Query strings land in proxy logs, browser history and
/// Referer headers; only the WebSocket handshake gets that fallback.
fn authorized(state: &AppState, headers: &HeaderMap) -> bool {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| token_matches(t, &state.token))
}

/// The auth layer over every HTTP route: a 401 is written from the request
/// head alone, before any body is read.
async fn require_auth(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if !authorized(&state, request.headers()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(request).await
}

/// The WebSocket gate: the header if the client can set one, `?token=` for
/// browsers, which cannot on a handshake.
fn authorized_ws(state: &AppState, headers: &HeaderMap, query: &HashMap<String, String>) -> bool {
    authorized(state, headers)
        || query.get("token").is_some_and(|t| token_matches(t, &state.token))
}

async fn ws_sync(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if !authorized_ws(&state, &headers, &query) {
        warn!("sync connection rejected: bad or missing token");
        return StatusCode::UNAUTHORIZED.into_response();
    }
    ws.on_upgrade(move |socket| handle_sync(state, socket))
}

// StoreError is a fat enum; one Result per WS message is nowhere near hot.
#[allow(clippy::result_large_err)]
async fn handle_sync(state: AppState, mut socket: WebSocket) {
    let mut peer = {
        let store = state.store.lock().await;
        match Peer::start(&store, false) {
            Ok(peer) => peer,
            Err(e) => {
                warn!("failed to start sync session: {e}");
                return;
            }
        }
    };

    while let Some(incoming) = socket.recv().await {
        let text = match incoming {
            Ok(WsMessage::Text(text)) => text,
            Ok(WsMessage::Close(_)) | Err(_) => break,
            Ok(_) => continue,
        };
        let reply = {
            let mut store = state.store.lock().await;
            WireMsg::from_json(&text).and_then(|msg| peer.handle(&mut store, &msg))
        };
        match reply {
            Ok(Some(reply)) => {
                let done = matches!(reply, WireMsg::Done);
                if socket.send(WsMessage::Text(reply.to_json().into())).await.is_err() {
                    break;
                }
                if done {
                    break;
                }
            }
            Ok(None) => break,
            Err(e) => {
                warn!("sync session error: {e}");
                let bye = WireMsg::Error { message: e.to_string() };
                let _ = socket.send(WsMessage::Text(bye.to_json().into())).await;
                break;
            }
        }
    }

    let outcome = peer.outcome().clone();
    if !outcome.docs_updated.is_empty() || outcome.log_added > 0 || outcome.threads_added > 0 {
        let docs: Vec<&str> = outcome.docs_updated.iter().map(String::as_str).collect();
        let message = format!(
            "sync: {} ({} log entries, {} thread messages)",
            if docs.is_empty() { "no doc changes".to_string() } else { docs.join(", ") },
            outcome.log_added,
            outcome.threads_added,
        );
        let mut store = state.store.lock().await;
        if let Err(e) = store.export(&message) {
            warn!("export after sync failed: {e}");
        }
    }
    info!(
        "sync session done: {} docs updated, {} log entries in/{} out, {} thread messages in/{} out",
        outcome.docs_updated.len(),
        outcome.log_added,
        outcome.log_sent,
        outcome.threads_added,
        outcome.threads_sent,
    );
}
