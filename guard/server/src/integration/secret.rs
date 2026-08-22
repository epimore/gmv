use base::base64::Engine;
use base::rand::RngCore;
use base::tokio::sync::{RwLock, RwLockWriteGuard};
use base::utils::crypto::Aes256GcmCipher;
use std::sync::Arc;

use crate::core::{GuardError, GuardResult};

#[derive(Clone)]
pub struct IntegrationSecretCipher {
    cipher: Aes256GcmCipher,
}

impl std::fmt::Debug for IntegrationSecretCipher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IntegrationSecretCipher")
            .field("key", &"<redacted>")
            .finish()
    }
}

impl IntegrationSecretCipher {
    pub fn random_key_material() -> String {
        let mut key = [0_u8; 32];
        base::rand::rngs::OsRng.fill_bytes(&mut key);
        base::base64::engine::general_purpose::STANDARD_NO_PAD.encode(key)
    }

    pub fn from_base64_key_no_pad(key: &str) -> GuardResult<Self> {
        let cipher = Aes256GcmCipher::from_base64_key_no_pad(key)
            .map_err(|error| GuardError::InvalidConfig(error.to_string()))?;
        Ok(Self { cipher })
    }

    pub fn encrypt(&self, secret: &str) -> GuardResult<String> {
        self.cipher
            .encrypt_to_base64_no_pad(secret)
            .map_err(|error| GuardError::Conflict(format!("encrypt integration secret: {error}")))
    }

    pub fn decrypt(&self, ciphertext: &str) -> GuardResult<String> {
        self.cipher
            .decrypt_from_base64_no_pad(ciphertext)
            .map_err(|error| GuardError::Conflict(format!("decrypt integration secret: {error}")))
    }
}

#[derive(Debug, Clone)]
pub struct IntegrationSecretManager {
    cipher: Arc<RwLock<IntegrationSecretCipher>>,
}

impl IntegrationSecretManager {
    pub fn new(cipher: IntegrationSecretCipher) -> Self {
        Self {
            cipher: Arc::new(RwLock::new(cipher)),
        }
    }

    pub async fn encrypt(&self, secret: &str) -> GuardResult<String> {
        self.cipher.read().await.encrypt(secret)
    }

    pub async fn decrypt(&self, ciphertext: &str) -> GuardResult<String> {
        self.cipher.read().await.decrypt(ciphertext)
    }

    pub(crate) async fn write(&self) -> RwLockWriteGuard<'_, IntegrationSecretCipher> {
        self.cipher.write().await
    }
}
