use super::*;
use gmv_protocol::common::v1::{AccessGrant, NodeKind};

#[test]
fn public_address_filter_rejects_internal_and_documentation_networks() {
    assert!(!is_public_address("127.0.0.1".parse().unwrap()));
    assert!(!is_public_address("10.0.0.1".parse().unwrap()));
    assert!(!is_public_address("169.254.1.1".parse().unwrap()));
    assert!(!is_public_address("192.0.2.1".parse().unwrap()));
    assert!(!is_public_address("2001:db8::1".parse().unwrap()));
    assert!(is_public_address("8.8.8.8".parse().unwrap()));
}

#[test]
fn url_identity_does_not_persist_sensitive_query_or_fragment() {
    assert_eq!(
        stable_url_identity("https://example.com/image.jpg?token=secret#x").unwrap(),
        "https://example.com/image.jpg"
    );
}

#[test]
fn credentialed_redirect_origin_comparison_includes_scheme_host_and_port() {
    let source = Url::parse("https://session.internal:8443/image/1").unwrap();
    assert!(same_origin(
        &source,
        &Url::parse("https://session.internal:8443/image/2").unwrap()
    ));
    assert!(!same_origin(
        &source,
        &Url::parse("https://other.internal:8443/image/2").unwrap()
    ));
    assert!(!same_origin(
        &source,
        &Url::parse("http://session.internal:8443/image/2").unwrap()
    ));
    assert!(!same_origin(
        &source,
        &Url::parse("https://session.internal:9443/image/2").unwrap()
    ));
}

#[test]
fn grant_rejects_expired_wrong_audience_and_wrong_purpose() {
    let identity = NodeIdentity {
        node_id: "avai-1".to_string(),
        instance_id: "instance-1".to_string(),
        kind: NodeKind::Avai as i32,
    };
    let grant = |expected: NodeIdentity, purpose: &str, expires_at_epoch_ms| AccessGrant {
        grant_id: "grant-1".to_string(),
        expected_consumer: Some(expected),
        purpose: purpose.to_string(),
        expires_at_epoch_ms,
        endpoints: Vec::new(),
        proof: vec![1],
    };
    assert_eq!(
        validate_grant(
            &grant(identity.clone(), "capability", 99),
            &identity,
            "capability",
            100
        )
        .unwrap_err()
        .code,
        "source_expired"
    );
    let mut other = identity.clone();
    other.instance_id = "instance-2".to_string();
    assert_eq!(
        validate_grant(
            &grant(other, "capability", 101),
            &identity,
            "capability",
            100
        )
        .unwrap_err()
        .code,
        "source_fetch_denied"
    );
    assert_eq!(
        validate_grant(
            &grant(identity.clone(), "other", 101),
            &identity,
            "capability",
            100
        )
        .unwrap_err()
        .code,
        "source_fetch_denied"
    );
}

#[test]
fn image_validation_rejects_metadata_mismatch() {
    use base::base64::Engine;
    let bytes = base::base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")
        .unwrap();
    let fetched = || FetchedBody {
        bytes: Bytes::from(bytes.clone()),
        content_type: Some("image/png".to_string()),
    };
    let wrong_hash = ImageMetadata {
        sha256: "wrong".to_string(),
        ..ImageMetadata::default()
    };
    assert_eq!(
        validate_image(fetched(), Some(&wrong_hash), "test".to_string())
            .unwrap_err()
            .code,
        "source_hash_mismatch"
    );
    let wrong_size = ImageMetadata {
        size_bytes: bytes.len() as u64 + 1,
        ..ImageMetadata::default()
    };
    assert_eq!(
        validate_image(fetched(), Some(&wrong_size), "test".to_string())
            .unwrap_err()
            .code,
        "source_size_mismatch"
    );
}
