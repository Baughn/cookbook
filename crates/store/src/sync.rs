//! Replica sync: the Automerge sync protocol per doc, plus append-merge for
//! the cook log and thread messages, spoken as JSON rounds over any
//! transport.
//!
//! The protocol is deliberately dumb: strict alternation. The initiator
//! sends a round (sync messages for every doc it knows, plus its log and
//! thread uids); the responder replies in kind; rounds ping-pong until the
//! initiator sees an empty round in both directions and says `done`, which
//! the responder echoes. Docs the other side has never heard of simply show up as sync
//! messages for unknown ids and get created. Everything received is
//! persisted after each round, so an interrupted sync loses nothing and the
//! next session picks up where things stand.
//!
//! [`Peer`] is sans-IO: both the server and any client drive it by shuttling
//! [`WireMsg`] values over whatever pipe they have. Tests drive two peers
//! with no transport at all.

use std::collections::{BTreeMap, BTreeSet};

use automerge::sync::{Message, State as SyncState, SyncDoc};
use automerge::{AutoCommit, Change, ChangeHash};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use mise_core::types::LogEntry;
use serde::{Deserialize, Serialize};

use crate::docid::DocId;
use crate::error::{Result, StoreError};
use crate::store::Store;
use crate::threads::ThreadMessage;

// ------------------------------------------------------------------ wire --

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum WireMsg {
    Round(Round),
    Done,
    Error { message: String },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Round {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub docs: Vec<DocMsg>,
    /// Sent once per side, in its first round.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_uids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub log_entries: Vec<LogRow>,
    /// Sent once per side, alongside `log_uids`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_uids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub thread_entries: Vec<ThreadRow>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocMsg {
    pub doc: String,
    /// Base64 of an encoded Automerge sync message.
    pub data: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogRow {
    pub uid: String,
    pub entry: LogEntry,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThreadRow {
    pub uid: String,
    pub message: ThreadMessage,
}

impl WireMsg {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("wire messages serialize")
    }

    pub fn from_json(s: &str) -> Result<WireMsg> {
        serde_json::from_str(s).map_err(StoreError::Json)
    }
}

// ------------------------------------------------------------------ peer --

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    /// Local docs that received new changes (created or updated).
    pub docs_updated: BTreeSet<String>,
    /// Log entries added locally.
    pub log_added: usize,
    /// Log entries sent to the peer.
    pub log_sent: usize,
    /// Thread messages added locally.
    pub threads_added: usize,
    /// Thread messages sent to the peer.
    pub threads_sent: usize,
}

struct DocPeer {
    doc: AutoCommit,
    state: SyncState,
    /// Heads whose changes are already persisted locally.
    baseline: Vec<ChangeHash>,
}

/// One side of a sync session. Feed it incoming [`WireMsg`]s with
/// [`Peer::handle`]; ship whatever it returns. `None` means the session is
/// complete and the connection can close.
pub struct Peer {
    initiator: bool,
    docs: BTreeMap<String, DocPeer>,
    /// Covers both the log and thread uid exchanges — they travel together.
    sent_uids: bool,
    pending_entries: Option<Vec<LogRow>>,
    pending_threads: Option<Vec<ThreadRow>>,
    outcome: SyncOutcome,
}

impl Peer {
    pub fn start(store: &Store, initiator: bool) -> Result<Peer> {
        let mut docs = BTreeMap::new();
        for id in store.all_doc_ids()? {
            let mut doc = store.load_doc(&DocId::parse(&id)?)?;
            let baseline = doc.get_heads();
            docs.insert(id, DocPeer { doc, state: SyncState::new(), baseline });
        }
        Ok(Peer {
            initiator,
            docs,
            sent_uids: false,
            pending_entries: None,
            pending_threads: None,
            outcome: SyncOutcome::default(),
        })
    }

    /// The initiator's opening round: sync messages for every doc it knows,
    /// plus its log and thread uids.
    pub fn initial_round(&mut self, store: &Store) -> Result<WireMsg> {
        assert!(self.initiator, "only the initiator opens");
        let docs = self.generate_all();
        self.sent_uids = true;
        Ok(WireMsg::Round(Round {
            docs,
            log_uids: Some(store.log_uids()?),
            log_entries: vec![],
            thread_uids: Some(store.thread_uids()?),
            thread_entries: vec![],
        }))
    }

    pub fn outcome(&self) -> &SyncOutcome {
        &self.outcome
    }

    fn generate_all(&mut self) -> Vec<DocMsg> {
        self.docs
            .iter_mut()
            .filter_map(|(id, dp)| {
                dp.doc
                    .sync()
                    .generate_sync_message(&mut dp.state)
                    .map(|m| DocMsg { doc: id.clone(), data: B64.encode(m.encode()) })
            })
            .collect()
    }

    /// Persist everything received so far: new changes per doc (hash-deduped,
    /// doc rows created for docs sync introduced), so an interrupted session
    /// loses nothing.
    fn commit(&mut self, store: &mut Store) -> Result<()> {
        for (id, dp) in self.docs.iter_mut() {
            let changes: Vec<Change> =
                dp.doc.get_changes(&dp.baseline).into_iter().cloned().collect();
            if changes.is_empty() {
                continue;
            }
            store.ensure_doc_row(&DocId::parse(id)?)?;
            if store.append_changes(id, &changes, &mut dp.doc)? > 0 {
                self.outcome.docs_updated.insert(id.clone());
            }
            dp.baseline = dp.doc.get_heads();
        }
        Ok(())
    }

    pub fn handle(&mut self, store: &mut Store, msg: &WireMsg) -> Result<Option<WireMsg>> {
        match msg {
            WireMsg::Error { message } => {
                Err(StoreError::Corrupt(format!("peer reported: {message}")))
            }
            WireMsg::Done => {
                // Responder echoes the goodbye; initiator hangs up.
                if self.initiator {
                    Ok(None)
                } else {
                    Ok(Some(WireMsg::Done))
                }
            }
            WireMsg::Round(round) => {
                let incoming_payload = !round.docs.is_empty()
                    || !round.log_entries.is_empty()
                    || !round.thread_entries.is_empty();

                for dm in &round.docs {
                    let id = DocId::parse(&dm.doc)?;
                    let dp = self.docs.entry(id.to_string()).or_insert_with(|| DocPeer {
                        doc: AutoCommit::new(),
                        state: SyncState::new(),
                        baseline: vec![],
                    });
                    let message = Message::decode(&B64.decode(&dm.data).map_err(|e| {
                        StoreError::Corrupt(format!("bad sync message encoding: {e}"))
                    })?)
                    .map_err(|e| StoreError::Corrupt(format!("bad sync message: {e}")))?;
                    dp.doc.sync().receive_sync_message(&mut dp.state, message)?;
                }
                for row in &round.log_entries {
                    if store.insert_log_row(&row.uid, &row.entry)? {
                        self.outcome.log_added += 1;
                    }
                }
                for row in &round.thread_entries {
                    if store.insert_thread_row(&row.uid, &row.message)? {
                        self.outcome.threads_added += 1;
                    }
                }
                self.commit(store)?;

                if let Some(uids) = &round.log_uids {
                    let theirs: BTreeSet<&String> = uids.iter().collect();
                    let missing: Vec<LogRow> = store
                        .log_rows()?
                        .into_iter()
                        .filter(|(uid, _)| !theirs.contains(uid))
                        .map(|(uid, entry)| LogRow { uid, entry })
                        .collect();
                    self.pending_entries = Some(missing);
                }
                if let Some(uids) = &round.thread_uids {
                    let theirs: BTreeSet<&String> = uids.iter().collect();
                    let missing: Vec<ThreadRow> = store
                        .thread_rows()?
                        .into_iter()
                        .filter(|(uid, _)| !theirs.contains(uid))
                        .map(|(uid, message)| ThreadRow { uid, message })
                        .collect();
                    self.pending_threads = Some(missing);
                }

                let docs = self.generate_all();
                let (log_uids, thread_uids) = if self.sent_uids {
                    (None, None)
                } else {
                    self.sent_uids = true;
                    (Some(store.log_uids()?), Some(store.thread_uids()?))
                };
                let log_entries = self.pending_entries.take().unwrap_or_default();
                self.outcome.log_sent += log_entries.len();
                let thread_entries = self.pending_threads.take().unwrap_or_default();
                self.outcome.threads_sent += thread_entries.len();

                let reply_empty = docs.is_empty()
                    && log_uids.is_none()
                    && log_entries.is_empty()
                    && thread_entries.is_empty();
                if self.initiator && reply_empty && !incoming_payload {
                    Ok(Some(WireMsg::Done))
                } else {
                    Ok(Some(WireMsg::Round(Round {
                        docs,
                        log_uids,
                        log_entries,
                        thread_uids,
                        thread_entries,
                    })))
                }
            }
        }
    }
}
