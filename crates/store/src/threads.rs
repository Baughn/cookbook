//! Threads: append-only conversation transcripts, one per page plus the
//! global planning thread. Like the cook log they are plain SQLite rows,
//! not CRDTs — cross-replica identity is a content-hash uid, and merge is
//! set union via the sync uid exchange.
//!
//! Threads store *text turns only* (user and assistant). Tool activity is
//! not transcribed: page edits carry their own provenance in the Automerge
//! history, and a resumed conversation re-reads pages through tools rather
//! than trusting stale tool results.

use std::fmt;
use std::str::FromStr;

use jiff::civil::DateTime;
use serde::{Deserialize, Serialize};

use crate::docid::DocId;
use crate::error::StoreError;

/// Identity of one thread: the global planning assistant, or a page thread.
/// The string form doubles as the SQLite key and the export path under
/// `threads/`: `planning`, `recipe/mapo-tofu`, `location/home/pantry`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ThreadId {
    Planning,
    Page(DocId),
}

impl ThreadId {
    pub fn parse(s: &str) -> Result<ThreadId, StoreError> {
        match s {
            "planning" => Ok(ThreadId::Planning),
            other => Ok(ThreadId::Page(DocId::parse(other)?)),
        }
    }
}

impl fmt::Display for ThreadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ThreadId::Planning => write!(f, "planning"),
            ThreadId::Page(id) => write!(f, "{id}"),
        }
    }
}

impl Serialize for ThreadId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ThreadId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<ThreadId, D::Error> {
        let raw = String::deserialize(d)?;
        ThreadId::parse(&raw).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Role::User => write!(f, "user"),
            Role::Assistant => write!(f, "assistant"),
        }
    }
}

impl FromStr for Role {
    type Err = String;

    fn from_str(s: &str) -> Result<Role, String> {
        match s {
            "user" => Ok(Role::User),
            "assistant" => Ok(Role::Assistant),
            other => Err(format!("not a role: {other:?}")),
        }
    }
}

/// One turn in a thread. `content` is normalized on append: LF line endings,
/// trimmed, non-empty.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadMessage {
    pub thread: ThreadId,
    pub role: Role,
    pub content: String,
    pub created: DateTime,
}
