use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("automerge: {0}")]
    Automerge(#[from] automerge::AutomergeError),
    #[error("loading change: {0}")]
    LoadChange(#[from] automerge::LoadChangeError),
    #[error("hydrating document: {0}")]
    Hydrate(#[from] autosurgeon::HydrateError),
    #[error("reconciling document: {0}")]
    Reconcile(#[from] autosurgeon::ReconcileError),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("git {args:?}: {stderr}")]
    Git { args: Vec<String>, stderr: String },
    #[error("no such document: {0}")]
    NotFound(String),
    #[error("document already exists: {0}")]
    Exists(String),
    #[error("not a document id: {0:?}")]
    BadDocId(String),
    #[error("corpus already initialized at {0}")]
    AlreadyInitialized(PathBuf),
    #[error("no corpus at {0} (run `mise init`?)")]
    NoCorpus(PathBuf),
    #[error("corrupt corpus state: {0}")]
    Corrupt(String),
    #[error("invalid input: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;
