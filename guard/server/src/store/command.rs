use sha2::{Digest, Sha256};

use crate::core::{GuardError, GuardResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpCommandClaim {
    Claimed {
        command_id: String,
        operation_id: String,
    },
    Pending {
        operation_id: String,
    },
    Completed {
        operation_id: String,
        status: u16,
        response_body: Vec<u8>,
    },
}

pub fn http_command_id(integration_id: &str, request_id: &str) -> String {
    let digest = Sha256::digest(format!("{integration_id}\n{request_id}").as_bytes());
    format!("http:{}", hex::encode(digest))
}

pub fn validate_request_id(request_id: &str) -> GuardResult<()> {
    if request_id.is_empty()
        || request_id.len() > 128
        || request_id.chars().any(char::is_whitespace)
    {
        return Err(GuardError::InvalidConfig(
            "X-GMV-Request-ID must contain 1..=128 non-whitespace characters".to_string(),
        ));
    }
    Ok(())
}
