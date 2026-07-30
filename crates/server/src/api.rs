//! The JSON API the web app consumes. Read views are the same structures
//! the assistant's tools render from. Mutations are revert plus a small
//! allowlist of tap-shaped edits under `/api/edit/{action}` — each one
//! *is* the corresponding assistant tool (same validation, same
//! normalization, `ui:` provenance), so this never becomes a second
//! editing surface with its own rules. Prose stays conversational.

// Handlers speak the store's Result and map every error once in `fail`;
// the Err size is the store's business, and none of this is hot.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use jiff::Zoned;
use mise_assistant::views;
use mise_store::{DocId, StoreError};
use serde::Deserialize;
use serde_json::json;
use tracing::warn;

use crate::{AppState, authorized};

/// Uniform error mapping: what the caller got wrong is 4xx with a message,
/// the rest is a logged 500.
fn fail(e: StoreError) -> Response {
    let status = match &e {
        StoreError::NotFound(_) => StatusCode::NOT_FOUND,
        StoreError::Invalid(_) | StoreError::BadDocId(_) => StatusCode::BAD_REQUEST,
        _ => {
            warn!("api error: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    (status, Json(json!({"error": e.to_string()}))).into_response()
}

fn unauthorized() -> Response {
    StatusCode::UNAUTHORIZED.into_response()
}

pub(crate) async fn queue(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if !authorized(&state, &headers, &query) {
        return unauthorized();
    }
    let store = state.store.lock().await;
    match views::queue_view(&store, Zoned::now().datetime()) {
        Ok(view) => Json(view).into_response(),
        Err(e) => fail(e),
    }
}

pub(crate) async fn pages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if !authorized(&state, &headers, &query) {
        return unauthorized();
    }
    let store = state.store.lock().await;
    let listing = (|| -> Result<serde_json::Value, StoreError> {
        let corpus = store.corpus()?;
        let files = mise_store::render::render(&corpus);
        // Doc-backed paths get their doc id (that's the handle for history
        // and threads); recipes carry browse metadata.
        let mut doc_paths: HashMap<String, String> = HashMap::new();
        for id in known_doc_ids(&corpus) {
            doc_paths.insert(id.export_path(), id.to_string());
        }
        let pages: Vec<serde_json::Value> = files
            .keys()
            .map(|path| {
                let mut page = json!({"path": path});
                if let Some(doc) = doc_paths.get(path) {
                    page["doc"] = json!(doc);
                }
                if let Some(recipe) = path
                    .strip_prefix("recipes/")
                    .and_then(|p| p.strip_suffix(".md"))
                    .and_then(|slug| corpus.recipes.get(slug))
                {
                    page["title"] = json!(recipe.title);
                    page["tags"] = json!(recipe.tags);
                    page["effort"] = json!(recipe.effort);
                    page["status"] = json!(recipe.status);
                }
                if let Some(technique) = path
                    .strip_prefix("techniques/")
                    .and_then(|p| p.strip_suffix(".md"))
                    .and_then(|slug| corpus.techniques.get(slug))
                {
                    page["title"] = json!(technique.title);
                    page["tags"] = json!(technique.tags);
                }
                page
            })
            .collect();
        Ok(json!({"pages": pages}))
    })();
    match listing {
        Ok(v) => Json(v).into_response(),
        Err(e) => fail(e),
    }
}

fn known_doc_ids(corpus: &mise_store::pages::CorpusState) -> Vec<DocId> {
    let mut ids = vec![
        DocId::State,
        DocId::Queue,
        DocId::Someday,
        DocId::Shopping,
        DocId::Steering,
        DocId::Facts,
    ];
    let slug = |s: &str| mise_core::types::Slug::new(s).ok();
    for name in corpus.locations.keys() {
        if let Some(l) = slug(name) {
            ids.push(DocId::Pantry(l.clone()));
            ids.push(DocId::Equipment(l.clone()));
            ids.push(DocId::Shops(l.clone()));
            ids.push(DocId::Fridge(l));
        }
    }
    for name in corpus.recipes.keys() {
        if let Some(s) = slug(name) {
            ids.push(DocId::Recipe(s));
        }
    }
    for name in corpus.techniques.keys() {
        if let Some(s) = slug(name) {
            ids.push(DocId::Technique(s));
        }
    }
    ids
}

pub(crate) async fn page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
    Path(path): Path<String>,
) -> Response {
    if !authorized(&state, &headers, &query) {
        return unauthorized();
    }
    let store = state.store.lock().await;
    let content = (|| -> Result<Option<String>, StoreError> {
        let files = mise_store::render::render(&store.corpus()?);
        let key = path.trim_matches('/');
        let key = key.strip_suffix(".md").unwrap_or(key);
        Ok(files.get(&format!("{key}.md")).cloned())
    })();
    match content {
        Ok(Some(content)) => Json(json!({"path": path, "content": content})).into_response(),
        Ok(None) => fail(StoreError::NotFound(format!("page {path}"))),
        Err(e) => fail(e),
    }
}

pub(crate) async fn history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
    Path(doc): Path<String>,
) -> Response {
    if !authorized(&state, &headers, &query) {
        return unauthorized();
    }
    let store = state.store.lock().await;
    let result = DocId::parse(&doc).and_then(|id| store.history(&id));
    match result {
        Ok(changes) => Json(json!({
            "doc": doc,
            "changes": changes
                .iter()
                .map(|c| {
                    json!({
                        "hash": c.hash,
                        "message": c.message,
                        "time": c.time.map(|t| t.to_string()),
                    })
                })
                .collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(e) => fail(e),
    }
}

#[derive(Deserialize)]
pub(crate) struct RevertRequest {
    pub doc: String,
    pub hash: String,
}

pub(crate) async fn revert(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
    Json(request): Json<RevertRequest>,
) -> Response {
    if !authorized(&state, &headers, &query) {
        return unauthorized();
    }
    let mut store = state.store.lock().await;
    let result = DocId::parse(&request.doc).and_then(|id| {
        let short = &request.hash[..request.hash.len().min(8)];
        let message = format!("ui: revert {} to {short}", request.doc);
        store.revert(&id, &request.hash, &message, Zoned::now().timestamp())?;
        store.export(&message)
    });
    match result {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => fail(e),
    }
}

/// Item-list edits the UI may make directly, each mapped to the assistant
/// tool that implements it. Small, idempotent, timestamped at the edge —
/// tap-shaped, so an offline queue can replay them later. Deliberately
/// absent: anything with free text a conversation should own (recipe
/// bodies, queue reasons, steering).
const UI_ACTIONS: &[(&str, &str)] = &[
    ("pantry-set", "pantry_set"),
    ("pantry-remove", "pantry_remove"),
    ("equipment-set", "equipment_set"),
    ("equipment-remove", "equipment_remove"),
    ("fridge-add", "fridge_add"),
    ("fridge-remove", "fridge_remove"),
    ("shopping-add", "shopping_add"),
    ("shopping-update", "shopping_update"),
];

pub(crate) async fn edit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
    Path(action): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    use mise_assistant::tools::{self, ToolCtx};
    use mise_assistant::turn::ToolCall;

    if !authorized(&state, &headers, &query) {
        return unauthorized();
    }
    let (tool, input) = if action == "recipe-status" {
        // recipe_edit, narrowed to the status field: only these two keys
        // pass, so no payload can smuggle a body edit through.
        #[derive(Deserialize)]
        struct In {
            slug: String,
            status: String,
        }
        match serde_json::from_value::<In>(body) {
            Ok(a) => ("recipe_edit", json!({"slug": a.slug, "status": a.status})),
            Err(e) => {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": e.to_string()})))
                    .into_response();
            }
        }
    } else {
        match UI_ACTIONS.iter().find(|(name, _)| *name == action) {
            Some((_, tool)) => (*tool, body),
            None => {
                return fail(StoreError::NotFound(format!("edit action {action}")));
            }
        }
    };

    let mut store = state.store.lock().await;
    let ctx = ToolCtx { now: Zoned::now(), provenance: "ui".into() };
    let call = ToolCall { id: "ui".into(), name: tool.into(), input };
    match tools::execute(&mut store, &ctx, &call) {
        Ok(outcome) if outcome.is_error => {
            (StatusCode::BAD_REQUEST, Json(json!({"error": outcome.content}))).into_response()
        }
        Ok(outcome) => {
            if let Err(e) = store.export(&format!("ui: {}", outcome.content)) {
                return fail(e);
            }
            Json(json!({"ok": true, "result": outcome.content})).into_response()
        }
        Err(mise_assistant::AssistantError::Store(e)) => fail(*e),
        Err(e) => {
            warn!("edit action failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub(crate) async fn thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
    Path(thread): Path<String>,
) -> Response {
    if !authorized(&state, &headers, &query) {
        return unauthorized();
    }
    let store = state.store.lock().await;
    let result = mise_store::ThreadId::parse(&thread)
        .and_then(|id| store.thread_messages(&id));
    match result {
        Ok(messages) => Json(json!({
            "thread": thread,
            "messages": messages
                .iter()
                .map(|m| {
                    json!({
                        "role": m.role,
                        "content": m.content,
                        "created": m.created.to_string(),
                    })
                })
                .collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(e) => fail(e),
    }
}
