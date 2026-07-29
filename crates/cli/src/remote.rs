//! Remote mode: the saved server config and the client side of a sync
//! session. The CLI is the initiator Peer; the transport is a WebSocket.

use std::path::Path;

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use mise_store::Store;
use mise_store::sync::{Peer, SyncOutcome, WireMsg};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message;

/// Saved in `<root>/remote.json`, mode 0600 — beside the corpus, never in
/// the export.
#[derive(Serialize, Deserialize)]
pub struct Remote {
    pub url: String,
    pub token: String,
}

pub fn load(root: &Path) -> Result<Option<Remote>> {
    let path = root.join("remote.json");
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path)?;
    Ok(Some(serde_json::from_str(&raw).context("parsing remote.json")?))
}

pub fn save(root: &Path, remote: &Remote) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(root.join("remote.json"))?;
    f.write_all(serde_json::to_string_pretty(remote)?.as_bytes())?;
    f.write_all(b"\n")?;
    Ok(())
}

/// Accept ws://, wss://, http://, https:// forms; default the path to /sync.
pub fn normalize_url(url: &str) -> Result<String> {
    let url = url.trim();
    let url = if let Some(rest) = url.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if let Some(rest) = url.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if url.starts_with("ws://") || url.starts_with("wss://") {
        url.to_string()
    } else {
        bail!("server URL must start with ws://, wss://, http://, or https://");
    };
    let after_scheme = url.split_once("://").expect("checked above").1;
    Ok(if after_scheme.contains('/') {
        url.trim_end_matches('/').to_string()
    } else {
        format!("{url}/sync")
    })
}

/// One full sync session against the server.
pub fn sync(store: &mut Store, url: &str, token: &str) -> Result<SyncOutcome> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(sync_async(store, url, token))
}

async fn sync_async(store: &mut Store, url: &str, token: &str) -> Result<SyncOutcome> {
    let mut request = url
        .into_client_request()
        .with_context(|| format!("bad server URL {url:?}"))?;
    request
        .headers_mut()
        .insert("authorization", format!("Bearer {token}").parse()?);
    let (mut ws, _) = connect_async(request)
        .await
        .with_context(|| format!("connecting to {url}"))?;

    let mut peer = Peer::start(store, true)?;
    let first = peer.initial_round(store)?;
    ws.send(Message::text(first.to_json())).await?;
    while let Some(incoming) = ws.next().await {
        match incoming.context("connection lost mid-sync (already-received data is saved)")? {
            Message::Text(text) => {
                let msg = WireMsg::from_json(&text)?;
                match peer.handle(store, &msg)? {
                    Some(reply) => ws.send(Message::text(reply.to_json())).await?,
                    None => break,
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    let _ = ws.close(None).await;
    Ok(peer.outcome().clone())
}

pub fn describe(outcome: &SyncOutcome) -> String {
    if *outcome == SyncOutcome::default() {
        return "already in sync".to_string();
    }
    let mut parts = Vec::new();
    if !outcome.docs_updated.is_empty() {
        parts.push(format!(
            "updated {}",
            outcome.docs_updated.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    if outcome.log_added > 0 {
        parts.push(format!("{} log entries in", outcome.log_added));
    }
    if outcome.log_sent > 0 {
        parts.push(format!("{} log entries out", outcome.log_sent));
    }
    if parts.is_empty() {
        parts.push("pushed local changes".to_string());
    }
    parts.join("; ")
}
