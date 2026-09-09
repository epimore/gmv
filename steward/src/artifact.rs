use crate::config::{StewardConfig, TrustedSigningKey};
use base::{
    artifact::{
        ArtifactDownloadPolicy, ArtifactError, ArtifactErrorKind, StagedArtifact,
        VerifiedArtifactRequest, download_verify_and_stage,
    },
    base64::Engine,
};
use gmv_protocol::steward::v1::ArtifactManifest;
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

#[derive(Clone)]
pub struct ArtifactService {
    policy: ArtifactDownloadPolicy,
    keys: HashMap<String, Vec<u8>>,
}

impl ArtifactService {
    pub fn from_config(config: &StewardConfig) -> Result<Self, String> {
        let keys = config
            .trusted_signing_keys
            .iter()
            .map(decode_key)
            .collect::<Result<HashMap<_, _>, _>>()?;
        Ok(Self {
            policy: ArtifactDownloadPolicy {
                staging_root: config.staging_root.clone(),
                allowed_hosts: config
                    .artifact_allowed_hosts
                    .iter()
                    .cloned()
                    .collect::<HashSet<_>>(),
                max_bytes: config.max_artifact_bytes,
                total_timeout: Duration::from_secs(config.artifact_timeout_secs),
                allow_loopback_http: config
                    .gmvc
                    .as_ref()
                    .is_some_and(|center| center.allow_plaintext),
            },
            keys,
        })
    }

    pub async fn stage(
        &self,
        manifest: &ArtifactManifest,
        cancel: base::tokio_util::sync::CancellationToken,
    ) -> Result<StagedArtifact, ArtifactError> {
        if manifest.platform != std::env::consts::OS
            || manifest.architecture != std::env::consts::ARCH
            || manifest.download_expires_at_epoch_ms <= crate::now_ms()
        {
            return Err(invalid(
                "artifact platform, architecture or expiry is incompatible",
            ));
        }
        let public_key = self
            .keys
            .get(&manifest.signing_key_id)
            .ok_or_else(|| invalid("artifact signing key is not trusted"))?;
        download_verify_and_stage(
            &VerifiedArtifactRequest {
                url: manifest.download_url.clone(),
                output_name: format!("{}-{}.bundle", manifest.artifact_id, manifest.revision),
                expected_size: manifest.content_size,
                expected_sha256: manifest.sha256.clone(),
                signature: manifest.signature.clone(),
                public_key: public_key.clone(),
            },
            &self.policy,
            cancel,
        )
        .await
    }
}

pub fn stable_error_code(error: &ArtifactError) -> &'static str {
    match error.kind() {
        ArtifactErrorKind::InvalidConfiguration => "artifact_invalid",
        ArtifactErrorKind::Denied => "artifact_denied",
        ArtifactErrorKind::Timeout => "artifact_timeout",
        ArtifactErrorKind::TooLarge => "artifact_too_large",
        ArtifactErrorKind::Integrity => "artifact_integrity_failed",
        ArtifactErrorKind::Signature => "artifact_signature_failed",
        ArtifactErrorKind::Io => "artifact_storage_failed",
        ArtifactErrorKind::Network => "artifact_download_failed",
        ArtifactErrorKind::Cancelled => "artifact_cancelled",
    }
}

fn decode_key(key: &TrustedSigningKey) -> Result<(String, Vec<u8>), String> {
    let bytes = base::base64::engine::general_purpose::STANDARD
        .decode(&key.public_key_base64)
        .map_err(|_| format!("invalid public key encoding: {}", key.key_id))?;
    if bytes.len() != 32 {
        return Err(format!("invalid public key length: {}", key.key_id));
    }
    Ok((key.key_id.clone(), bytes))
}

fn invalid(message: &str) -> ArtifactError {
    // Public artifact validation deliberately returns a stable kind; callers never expose paths.
    base::artifact::invalid_artifact_configuration(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CenterConfig, StewardConfig, TrustedSigningKey};
    use base::sha2::{Digest, Sha256};
    use ed25519_dalek::{Signer, SigningKey};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn serve_once(body: &'static [u8]) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request).await;
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            stream.write_all(body).await.unwrap();
        });
        format!("http://{address}/bundle")
    }

    fn config(staging_root: std::path::PathBuf, public_key: &[u8]) -> StewardConfig {
        StewardConfig {
            installation_id: "installation-1".to_string(),
            node_id: "steward-1".to_string(),
            manifest_path: staging_root.join("unused-manifest.yml"),
            database_path: staging_root.join("unused.db"),
            staging_root,
            max_artifact_bytes: 1024,
            artifact_timeout_secs: 5,
            artifact_allowed_hosts: vec!["127.0.0.1".to_string()],
            trusted_signing_keys: vec![TrustedSigningKey {
                key_id: "test-key".to_string(),
                public_key_base64: base::base64::engine::general_purpose::STANDARD
                    .encode(public_key),
            }],
            guard_endpoint: None,
            gmvc: Some(CenterConfig {
                host: "127.0.0.1".to_string(),
                port: 1883,
                username: None,
                password: None,
                keep_alive_secs: 30,
                request_capacity: 32,
                session_expiry_secs: 3600,
                topic_prefix: gmv_protocol::steward_mqtt::DEFAULT_TOPIC_PREFIX.to_string(),
                allow_plaintext: true,
                tls: None,
            }),
        }
    }

    fn manifest(body: &[u8], url: String, signing_key: &SigningKey) -> ArtifactManifest {
        ArtifactManifest {
            artifact_id: "gmv".to_string(),
            version: "1.0.0".to_string(),
            revision: "revision-1".to_string(),
            platform: std::env::consts::OS.to_string(),
            architecture: std::env::consts::ARCH.to_string(),
            content_size: body.len() as u64,
            sha256: format!("{:x}", Sha256::digest(body)),
            signature: signing_key.sign(body).to_bytes().to_vec(),
            signing_key_id: "test-key".to_string(),
            download_url: url,
            download_expires_at_epoch_ms: crate::now_ms() + 30_000,
            ..ArtifactManifest::default()
        }
    }

    #[tokio::test]
    async fn stages_a_signed_bundle_and_rejects_a_bad_signature() {
        let root = std::env::temp_dir().join(format!("steward-artifact-{}", uuid::Uuid::now_v7()));
        let signing_key = SigningKey::from_bytes(&[7; 32]);
        let service = ArtifactService::from_config(&config(
            root.clone(),
            signing_key.verifying_key().as_bytes(),
        ))
        .unwrap();
        let body = b"signed test bundle";
        let staged = service
            .stage(
                &manifest(body, serve_once(body).await, &signing_key),
                base::tokio_util::sync::CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(base::tokio::fs::read(&staged.path).await.unwrap(), body);

        let other_body = b"another bundle";
        let mut invalid = manifest(other_body, serve_once(other_body).await, &signing_key);
        invalid.revision = "revision-2".to_string();
        invalid.signature[0] ^= 0xff;
        let error = service
            .stage(&invalid, base::tokio_util::sync::CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(stable_error_code(&error), "artifact_signature_failed");
        assert!(!root.join("gmv-revision-2.bundle").exists());

        std::fs::remove_dir_all(root).unwrap();
    }
}
