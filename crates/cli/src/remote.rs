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

    // Write a fresh temp file and rename over the target. `mode` on
    // OpenOptions is the open(2) mode argument, which the kernel applies only
    // when the call creates the file — rewriting an existing remote.json kept
    // whatever mode it already had, so a copy restored from a tarball or an
    // `cp` stayed world-readable with the bearer token in cleartext. Renaming
    // over the target also makes the write atomic, so an interrupted save
    // cannot leave a half-written config where a token used to be.
    let path = root.join("remote.json");
    let tmp = root.join("remote.json.tmp");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(serde_json::to_string_pretty(remote)?.as_bytes())?;
    f.write_all(b"\n")?;
    f.sync_all()?;
    drop(f);
    // Belt and braces: if the temp file somehow predates us, its mode is ours.
    std::fs::set_permissions(&tmp, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    std::fs::rename(&tmp, &path)?;
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
    // The peer's schema is something we learned, not something we moved, so
    // an idempotent re-sync still reads as "already in sync" — but say so
    // when the other side writes a shape this build has never heard of.
    let stale = if outcome.peer_is_newer() {
        format!(
            " (the server writes schema {}, this build reads {} — upgrade to see everything)",
            outcome.peer_schema.unwrap_or_default(),
            mise_store::pages::SCHEMA_VERSION,
        )
    } else {
        String::new()
    };
    if outcome.is_empty() {
        return format!("already in sync{stale}");
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
    if outcome.threads_added > 0 {
        parts.push(format!("{} thread messages in", outcome.threads_added));
    }
    if outcome.threads_sent > 0 {
        parts.push(format!("{} thread messages out", outcome.threads_sent));
    }
    if parts.is_empty() {
        parts.push("pushed local changes".to_string());
    }
    format!("{}{stale}", parts.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn remote() -> Remote {
        Remote { url: "wss://cook.example.com/sync".into(), token: "0123456789abcdef".into() }
    }

    #[test]
    fn a_fresh_save_is_private() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), &remote()).unwrap();
        assert_eq!(mode_of(&dir.path().join("remote.json")), 0o600);
    }

    /// The case that was broken: `mode` on OpenOptions is only honoured when
    /// open(2) creates the file, so rewriting a remote.json restored from a
    /// tarball (or copied with cp) left the bearer token world-readable.
    #[test]
    fn saving_over_a_loose_file_tightens_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote.json");
        std::fs::write(&path, "{}").unwrap();
        std::fs::set_permissions(&path, PermissionsExt::from_mode(0o644)).unwrap();
        assert_eq!(mode_of(&path), 0o644, "precondition: the file starts loose");

        save(dir.path(), &remote()).unwrap();

        assert_eq!(mode_of(&path), 0o600);
        assert_eq!(load(dir.path()).unwrap().unwrap().token, "0123456789abcdef");
    }

    #[test]
    fn saving_leaves_no_temp_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), &remote()).unwrap();
        assert!(!dir.path().join("remote.json.tmp").exists());
    }
}
