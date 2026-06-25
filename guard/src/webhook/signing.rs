use base::utils::crypto::hmac_sha256_hex;

use crate::core::{GuardError, GuardResult};

pub fn sign(secret: &[u8], timestamp_ms: i64, payload: &[u8]) -> GuardResult<String> {
    let timestamp = timestamp_ms.to_string();
    hmac_sha256_hex(secret, &[timestamp.as_bytes(), b".", payload])
        .map_err(|error| GuardError::InvalidConfig(format!("invalid HMAC key: {error}")))
}
