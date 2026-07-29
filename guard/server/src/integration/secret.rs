use base::utils::crypto::Aes256GcmCipher;

use crate::core::{GuardError, GuardResult};

pub const INTEGRATION_MASTER_KEY_CONFIG: &str = "guard.integrations.master_key";

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
