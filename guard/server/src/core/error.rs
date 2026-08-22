use std::collections::BTreeMap;

use base::exception::GlobalError;
use thiserror::Error;

pub use gmv_nodec::error_code::GmvErrorCode as GmvGuardErrorCode;

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
    #[error("{message}")]
    UserVisible {
        code: String,
        message: String,
        user_message: String,
        retryable: bool,
        details: BTreeMap<String, String>,
    },
}

impl GuardError {
    pub fn user_visible(
        code: impl Into<String>,
        message: impl Into<String>,
        user_message: impl Into<String>,
        retryable: bool,
        details: BTreeMap<String, String>,
    ) -> Self {
        Self::UserVisible {
            code: code.into(),
            message: message.into(),
            user_message: user_message.into(),
            retryable,
            details,
        }
    }
}

impl From<GlobalError> for GuardError {
    fn from(error: GlobalError) -> Self {
        let output = base::err::global_error_output(&error);
        let code = GmvGuardErrorCode::from_code(output.code)
            .map(|code| code.api_code().to_string())
            .unwrap_or_else(|| output.code_name.to_string());
        Self::UserVisible {
            code,
            message: error.to_string(),
            user_message: output.user_message.into_owned(),
            retryable: output.retryable,
            details: BTreeMap::new(),
        }
    }
}
