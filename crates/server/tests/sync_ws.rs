//! The M2 deliverable, end to end over real WebSockets: two clients
//! converging through the server, offline edits included — plus auth.

mod support;

use std::collections::BTreeMap;

use futures_util::{SinkExt, StreamExt};
use jiff::civil::Date;
use mise_core::types::{CookKind, LogEntry};
use mise_store::pages::{DishRefDoc, PantryDoc, PantryItemDoc, QueueDoc, QueueEntryDoc};
use mise_store::render::render;
use mise_store::sync::{Peer, SyncOutcome, WireMsg};
use mise_store::{DocId, Store};
use support::{Server, TOKEN, WRONG_TOKEN, empty, slug};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::connect_async;

/// The client side of a session: initiator Peer over a real socket.
async fn client_sync(store: &mut Store, url: &str, token: &str) -> SyncOutcome {
    let mut request = url.into_client_request().unwrap();
    if !token.is_empty() {
        request
            .headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
    }
    let (mut ws, _) = connect_async(request).await.unwrap();

    let mut peer = Peer::start(store, true).unwrap();
    let first = peer.initial_round(store).unwrap();
    ws.send(Message::text(first.to_json())).await.unwrap();
    while let Some(incoming) = ws.next().await {
        match incoming.unwrap() {
            Message::Text(text) => {
                let msg = WireMsg::from_json(&text).unwrap();
                match peer.handle(store, &msg).unwrap() {
                    Some(reply) => ws.send(Message::text(reply.to_json())).await.unwrap(),
                    None => break,
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    let _ = ws.close(None).await;
    peer.outcome().clone()
}

#[tokio::test]
async fn two_clients_converge_through_the_server() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::spawn(empty(dir.path())).await;
    let url = server.ws_url("/sync");

    // Two fresh devices pull the corpus.
    let mut a = Store::create_bare(&dir.path().join("a")).unwrap();
    let mut b = Store::create_bare(&dir.path().join("b")).unwrap();
    let out = client_sync(&mut a, &url, TOKEN).await;
    assert!(out.docs_updated.contains("state"), "{out:?}");
    client_sync(&mut b, &url, TOKEN).await;
    assert_eq!(a.corpus().unwrap(), b.corpus().unwrap());

    // Offline, divergent edits on both devices.
    a.modify::<PantryDoc>(&DocId::Pantry(slug("home")), "offline on a", jiff::Timestamp::UNIX_EPOCH, |p| {
        p.items.insert(
            "miso".into(),
            PantryItemDoc {
                name: "miso".into(),
                presence: mise_core::types::Presence::Out,
                bought: None,
                tier: Some(mise_core::types::Slug::new("town").unwrap()),
                note: None,
            },
        );
    })
    .unwrap();
    a.append_log(&LogEntry {
        date: Date::constant(2026, 7, 29),
        kind: CookKind::Meal,
        recipe: None,
        title: "Mapo tofu".into(),
        location: "home".into(),
        servings: 4,
        verdict: "great".into(),
        tags: BTreeMap::new(),
    }, "test: log", jiff::Timestamp::UNIX_EPOCH)
    .unwrap();
    b.modify::<QueueDoc>(&DocId::Queue, "offline on b", jiff::Timestamp::UNIX_EPOCH, |q| {
        q.entries.insert(
            "duck-curry".into(),
            QueueEntryDoc {
                dishes: vec![DishRefDoc { recipe: None, title: "Duck curry".into() }],
                reason: Some("basement idea".into()),
                added: "2026-07-29".into(),
            },
        );
    })
    .unwrap();

    // Signal returns: everyone syncs (A twice, to pick up B's edits).
    client_sync(&mut a, &url, TOKEN).await;
    client_sync(&mut b, &url, TOKEN).await;
    client_sync(&mut a, &url, TOKEN).await;

    let ca = a.corpus().unwrap();
    let cb = b.corpus().unwrap();
    assert_eq!(ca, cb);
    assert_eq!(ca.locations["home"].pantry.items["miso"].presence, mise_core::types::Presence::Out);
    assert!(ca.queue.entries.contains_key("duck-curry"));
    assert_eq!(ca.log.len(), 1);
    assert_eq!(render(&ca), render(&cb), "two devices, same files");

    // The server exported after syncing, with sync provenance.
    let server_export = dir.path().join("server/export");
    let git_log = std::process::Command::new("git")
        .arg("-C")
        .arg(&server_export)
        .args(["log", "--format=%s"])
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&git_log.stdout).into_owned();
    assert!(log.lines().any(|l| l.starts_with("sync: ")), "{log}");
    let queue_md = std::fs::read_to_string(server_export.join("queue.md")).unwrap();
    assert!(queue_md.contains("Duck curry"), "{queue_md}");
}

#[tokio::test]
async fn bad_token_is_rejected_at_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::spawn(empty(dir.path())).await;
    let url = server.ws_url("/sync");

    for token in [WRONG_TOKEN, ""] {
        let mut request = url.clone().into_client_request().unwrap();
        if !token.is_empty() {
            request
                .headers_mut()
                .insert("authorization", format!("Bearer {token}").parse().unwrap());
        }
        let err = connect_async(request).await.expect_err("upgrade must fail");
        let msg = err.to_string();
        assert!(msg.contains("401"), "{msg}");
    }
}

#[tokio::test]
async fn query_token_works_for_browserish_clients() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::spawn(empty(dir.path())).await;
    let mut a = Store::create_bare(&dir.path().join("a")).unwrap();
    let out = client_sync(&mut a, &server.ws_url(&format!("/sync?token={TOKEN}")), "").await;
    assert!(out.docs_updated.contains("queue"), "{out:?}");
}
