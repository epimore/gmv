use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use hmac::{Hmac, Mac};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

use crate::core::{GuardError, GuardResult};

const SIGNATURE_VERSION: &str = "GMV-HMAC-SHA256-V1";
const RATE_LIMIT_WINDOW_MS: i64 = 60_000;
const RATE_LIMIT_REQUESTS_PER_WINDOW: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedRequest<'a> {
    pub access_key: &'a str,
    pub timestamp_ms: i64,
    pub nonce: &'a str,
    pub method: &'a str,
    pub path: &'a str,
    pub query: &'a str,
    pub request_id: &'a str,
    pub body: &'a [u8],
}

impl SignedRequest<'_> {
    pub fn canonical(&self) -> String {
        format!(
            "{SIGNATURE_VERSION}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            self.access_key,
            self.timestamp_ms,
            self.nonce,
            self.method.to_ascii_uppercase(),
            self.path,
            canonical_query(self.query),
            self.request_id,
            body_sha256(self.body)
        )
    }
}

pub fn canonical_query(query: &str) -> String {
    let mut values = url::form_urlencoded::parse(query.as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    values.sort();
    values
        .into_iter()
        .map(|(key, value)| format!("{}={}", rfc3986_component(&key), rfc3986_component(&value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn rfc3986_component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

pub fn sign_request(secret: &[u8], request: &SignedRequest<'_>) -> GuardResult<String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret)
        .map_err(|_| GuardError::InvalidConfig("invalid HMAC secret".to_string()))?;
    mac.update(request.canonical().as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

pub fn verify_request(
    secret: &[u8],
    request: &SignedRequest<'_>,
    signature: &str,
) -> GuardResult<()> {
    let signature = hex::decode(signature)
        .map_err(|_| GuardError::InvalidIdentity("invalid integration signature".to_string()))?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret)
        .map_err(|_| GuardError::InvalidConfig("invalid HMAC secret".to_string()))?;
    mac.update(request.canonical().as_bytes());
    mac.verify_slice(&signature)
        .map_err(|_| GuardError::InvalidIdentity("invalid integration signature".to_string()))
}

pub fn body_sha256(body: &[u8]) -> String {
    hex::encode(Sha256::digest(body))
}

#[derive(Debug, Clone)]
pub struct HmacNonceCache {
    entries: Arc<Mutex<HashMap<(String, String), i64>>>,
    rate_entries: Arc<Mutex<HashMap<String, VecDeque<i64>>>>,
    ttl_ms: i64,
    capacity: usize,
}

impl HmacNonceCache {
    pub fn new(ttl_ms: i64, capacity: usize) -> GuardResult<Self> {
        if ttl_ms <= 0 || capacity == 0 {
            return Err(GuardError::InvalidConfig(
                "integration nonce cache policy is invalid".to_string(),
            ));
        }
        Ok(Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            rate_entries: Arc::new(Mutex::new(HashMap::new())),
            ttl_ms,
            capacity,
        })
    }

    pub fn claim_rate_slot(&self, access_key: &str, now_ms: i64) -> GuardResult<()> {
        let cutoff = now_ms.saturating_sub(RATE_LIMIT_WINDOW_MS);
        let mut entries = self.rate_entries.lock();
        let requests = entries.entry(access_key.to_string()).or_default();
        while requests
            .front()
            .is_some_and(|timestamp| *timestamp <= cutoff)
        {
            requests.pop_front();
        }
        if requests.len() >= RATE_LIMIT_REQUESTS_PER_WINDOW {
            return Err(GuardError::Capacity(
                "integration request rate limit exceeded".to_string(),
            ));
        }
        requests.push_back(now_ms);
        Ok(())
    }

    pub fn claim(&self, access_key: &str, nonce: &str, now_ms: i64) -> GuardResult<()> {
        if !(16..=64).contains(&nonce.len()) || !nonce.bytes().all(|value| value.is_ascii_graphic())
        {
            return Err(GuardError::InvalidIdentity(
                "invalid integration signature".to_string(),
            ));
        }
        let mut entries = self.entries.lock();
        entries.retain(|_, expires_at| *expires_at > now_ms);
        let key = (access_key.to_string(), nonce.to_string());
        if entries.contains_key(&key) {
            return Err(GuardError::InvalidIdentity(
                "invalid integration signature".to_string(),
            ));
        }
        if entries.len() >= self.capacity {
            return Err(GuardError::Capacity(
                "integration nonce cache capacity exceeded".to_string(),
            ));
        }
        entries.insert(key, now_ms.saturating_add(self.ttl_ms));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request<'a>(query: &'a str, body: &'a [u8]) -> SignedRequest<'a> {
        SignedRequest {
            access_key: "ak_test",
            timestamp_ms: 1_700_000_000_000,
            nonce: "0123456789abcdef",
            method: "post",
            path: "/openapi/v1/devices",
            query,
            request_id: "request-test-1",
            body,
        }
    }

    #[test]
    fn signature_covers_body_and_canonical_query() {
        let signature = sign_request(b"secret", &request("b=2&a=1", br#"{"ok":true}"#)).unwrap();
        verify_request(
            b"secret",
            &request("a=1&b=2", br#"{"ok":true}"#),
            &signature,
        )
        .unwrap();
        assert!(verify_request(b"secret", &request("a=1&b=2", b"{}"), &signature).is_err());
    }

    #[test]
    fn canonical_query_uses_rfc3986_and_preserves_duplicate_values() {
        assert_eq!(
            canonical_query("tag=z&name=%E4%B8%AD+%E6%96%87&tag=a%2Fb"),
            "name=%E4%B8%AD%20%E6%96%87&tag=a%2Fb&tag=z"
        );
    }

    #[test]
    fn published_cross_language_vector_is_stable() {
        let signature = sign_request(
            b"test-secret-32-bytes-0123456789",
            &SignedRequest {
                access_key: "ak_test_001",
                timestamp_ms: 1_700_000_000_000,
                nonce: "0123456789abcdef",
                method: "POST",
                path: "/openapi/v1/devices",
                query: "tag=z&name=%E4%B8%AD+%E6%96%87&tag=a%2Fb",
                request_id: "request-test-1",
                body: "{\"name\":\"摄像机 A\"}".as_bytes(),
            },
        )
        .unwrap();
        assert_eq!(
            signature,
            "927a91e9130182736dd2f825afdb0cb52f92dfd04d44b034b3d1b8d35cdf8e60"
        );
        assert!(include_str!("../../tests/fixtures/integration_hmac_v1.json").contains(&signature));
    }

    #[test]
    fn nonce_cache_rejects_replay_and_recovers_capacity_after_ttl() {
        let cache = HmacNonceCache::new(100, 1).unwrap();
        cache.claim("ak", "0123456789abcdef", 0).unwrap();
        assert!(cache.claim("ak", "0123456789abcdef", 1).is_err());
        cache.claim("ak", "fedcba9876543210", 101).unwrap();
    }

    #[test]
    fn rate_limit_is_per_access_key_and_recovers_after_window() {
        let cache = HmacNonceCache::new(100, 1).unwrap();
        for request in 0..RATE_LIMIT_REQUESTS_PER_WINDOW {
            cache.claim_rate_slot("ak-a", request as i64).unwrap();
        }
        assert!(cache.claim_rate_slot("ak-a", 1_000).is_err());
        cache.claim_rate_slot("ak-b", 1_000).unwrap();
        cache
            .claim_rate_slot("ak-a", RATE_LIMIT_WINDOW_MS + 1_000)
            .unwrap();
    }
}
