use thiserror::Error;

pub type GuardResult<T> = Result<T, GuardError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum GuardError {
    #[error("invalid config: {0}")]
    InvalidConfig(String),
    #[error("invalid identity: {0}")]
    InvalidIdentity(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("stale instance: {0}")]
    StaleInstance(String),
    #[error("capacity exceeded: {0}")]
    Capacity(String),
    #[error("time unsynced: {0}")]
    TimeUnsynced(String),
    #[error("duplicate event: {0}")]
    DuplicateEvent(String),
}
