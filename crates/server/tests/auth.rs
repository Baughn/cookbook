//! Auth is a property of the route table, not of individual handlers.
//! Every route is listed here with its access rule: bearer-authed (the
//! default — a new route lands behind the layer unless deliberately moved),
//! open (`/health`, the static app), or WebSocket (`/sync`, the one place
//! `?token=` is accepted because browsers cannot set handshake headers).

mod support;

use serde_json::{Value, json};
use support::{Server, TOKEN, WRONG_TOKEN, seeded};

/// Every bearer-authed route: method, path, and a well-formed body for the
/// POSTs. Auth must decide before the body shape is even looked at, so the
/// body's validity is irrelevant to the 401s — but a well-formed body keeps
/// the with-token sanity check meaningful.
fn authed_routes() -> Vec<(&'static str, &'static str, Option<Value>)> {
    vec![
        ("GET", "/api/queue", None),
        ("GET", "/api/pages", None),
        ("GET", "/api/page/queue", None),
        ("GET", "/api/history/queue", None),
        ("GET", "/api/location", None),
        ("GET", "/api/thread/planning", None),
        ("POST", "/api/revert", Some(json!({"doc": "queue", "hash": "deadbeef"}))),
        ("POST", "/api/edit/equipment-set", Some(json!({"item": "wok"}))),
        ("POST", "/chat", Some(json!({"message": "hi"}))),
    ]
}

async fn status_of(
    server: &Server,
    method: &str,
    path: &str,
    body: &Option<Value>,
    token: Option<&str>,
) -> u16 {
    let url = server.url(path);
    let mut req = match method {
        "GET" => server.client.get(url),
        "POST" => server.client.post(url).json(body.as_ref().expect("POST routes carry a body")),
        _ => unreachable!("route table only lists GET and POST"),
    };
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    req.send().await.unwrap().status().as_u16()
}

/// The missing header, the wrong token, and — since the fallback is scoped
/// to `/sync` — the right token in the query string all get the same 401.
#[tokio::test]
async fn every_authed_route_rejects_bad_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::spawn(seeded(dir.path())).await;

    for (method, path, body) in authed_routes() {
        let anon = status_of(&server, method, path, &body, None).await;
        assert_eq!(anon, 401, "{method} {path} without credentials");

        let wrong = status_of(&server, method, path, &body, Some(WRONG_TOKEN)).await;
        assert_eq!(wrong, 401, "{method} {path} with the wrong token");

        let query_path = format!("{path}?token={TOKEN}");
        let query = status_of(&server, method, &query_path, &body, None).await;
        assert_eq!(query, 401, "{method} {path} must not take a query token");

        let authed = status_of(&server, method, path, &body, Some(TOKEN)).await;
        assert_ne!(authed, 401, "{method} {path} with the real token");
    }
}

/// Auth runs on the request head. A garbage body on an unauthenticated
/// request is never read far enough to earn a 4xx of its own — the answer
/// is 401, not 400/413/415.
#[tokio::test]
async fn auth_is_decided_before_the_body_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::spawn(seeded(dir.path())).await;

    for path in ["/chat", "/api/revert", "/api/edit/equipment-set"] {
        let resp = server
            .client
            .post(server.url(path))
            .header("content-type", "application/json")
            .body("this is not json")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 401, "{path} with a malformed anonymous body");
    }

    // Oversized counts too: /chat's 12 MiB limit would answer 413, but an
    // unauthenticated sender never gets that far. The server may even hang
    // up while the body is still being written — a reset mid-upload is the
    // refusal happening before the read, which is the point.
    let oversized = "x".repeat(13 * 1024 * 1024);
    let outcome = server
        .client
        .post(server.url("/chat"))
        .header("content-type", "application/json")
        .body(oversized)
        .send()
        .await;
    match outcome {
        Ok(resp) => assert_eq!(resp.status().as_u16(), 401, "/chat oversized anonymous body"),
        Err(e) => assert!(e.is_request(), "hung up mid-upload, not a client bug: {e}"),
    }
}

/// `/health` and the static app stay open — the SPA must be able to render
/// its token prompt before it has a token to send.
#[tokio::test]
async fn health_and_the_static_app_need_no_token() {
    let dir = tempfile::tempdir().unwrap();
    let static_dir = dir.path().join("web");
    std::fs::create_dir_all(&static_dir).unwrap();
    std::fs::write(static_dir.join("index.html"), "<html>token prompt</html>").unwrap();
    let server = Server::spawn_with_static(seeded(dir.path()), static_dir).await;

    let health = server.get_anonymous("/health").await;
    assert_eq!(health.status().as_u16(), 200);

    let root = server.get_anonymous("/").await;
    assert_eq!(root.status().as_u16(), 200);

    // Unknown paths fall back to index.html for client-side routing.
    let spa_route = server.get_anonymous("/recipes/mapo-tofu").await;
    assert_eq!(spa_route.status().as_u16(), 200);
    assert!(spa_route.text().await.unwrap().contains("token prompt"));
}
