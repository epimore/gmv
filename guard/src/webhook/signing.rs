use base::sha2::Sha256;
use hmac::{Hmac, Mac};

use crate::core::{GuardError, GuardResult};

pub fn sign(secret: &[u8], timestamp_ms: i64, payload: &[u8]) -> GuardResult<String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret)
        .map_err(|error| GuardError::InvalidConfig(format!("invalid HMAC key: {error}")))?;
    mac.update(timestamp_ms.to_string().as_bytes());
    mac.update(b".");
    mac.update(payload);
    let bytes = mac.finalize().into_bytes();
    Ok(hex(&bytes))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}
