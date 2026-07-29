#[derive(Debug, thiserror::Error)]
pub enum AssistantError {
    #[error("store: {0}")]
    Store(Box<mise_store::StoreError>),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("api: {0}")]
    Api(String),
}

impl From<mise_store::StoreError> for AssistantError {
    fn from(e: mise_store::StoreError) -> AssistantError {
        AssistantError::Store(Box::new(e))
    }
}

pub type Result<T> = std::result::Result<T, AssistantError>;
