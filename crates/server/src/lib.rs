//! The Mise server: a thin, always-on replica. It holds the corpus, speaks
//! the sync protocol over WebSocket to any client, re-exports after every
//! session, and (from M3) will host the assistant. Caddy terminates TLS in
//! front of it; auth is a single static bearer token — one user, no
//! accounts, no sessions to expire.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::ws::{Message as WsMessage, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use mise_store::Store;
use mise_store::sync::{Peer, WireMsg};
use tokio::sync::Mutex;
use tracing::{info, warn};

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Mutex<Store>>,
    pub token: Arc<String>,
}

impl AppState {
    pub fn new(store: Store, token: String) -> AppState {
        AppState { store: Arc::new(Mutex::new(store)), token: Arc::new(token) }
    }
}

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/sync", get(ws_sync))
        .with_state(state)
}

/// Constant-time-ish comparison; the length is not treated as secret.
fn token_matches(provided: &str, expected: &str) -> bool {
    let (a, b) = (provided.as_bytes(), expected.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Bearer token from the Authorization header, or `?token=` for clients
/// that cannot set WebSocket headers (browsers).
fn authorized(state: &AppState, headers: &HeaderMap, query: &HashMap<String, String>) -> bool {
    let header_token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let provided = header_token.or_else(|| query.get("token").map(String::as_str));
    provided.is_some_and(|t| token_matches(t, &state.token))
}

async fn ws_sync(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if !authorized(&state, &headers, &query) {
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
