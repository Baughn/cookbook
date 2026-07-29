//! Corpus store: Automerge docs persisted in SQLite (the truth), plus the
//! deterministic read-only markdown export committed to a local git repo.
//!
//! The merge machinery is an internal detail: nothing outside this crate
//! sees Automerge types except through the typed page structs in [`pages`].

pub mod docid;
pub mod error;
pub mod pages;
pub mod render;
pub mod store;
pub mod sync;
pub mod threads;

pub use docid::DocId;
pub use error::StoreError;
pub use store::Store;
pub use threads::{Role, ThreadId, ThreadMessage};
