use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordVerifier};

use crate::auth::{Role, Secret};
use crate::core::{GuardError, GuardResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserProfile {
    pub username: String,
    pub role: Role,
    pub nickname: String,
    pub enabled: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone)]
pub struct UserAccount {
    pub username: String,
    pub role: Role,
    pub nickname: String,
    password_hash: Secret,
}

impl UserAccount {
    pub fn new(username: impl Into<String>, role: Role, password_hash: impl Into<String>) -> Self {
        Self::with_nickname(username, role, "", password_hash)
    }

    pub fn with_nickname(
        username: impl Into<String>,
        role: Role,
        nickname: impl Into<String>,
        password_hash: impl Into<String>,
    ) -> Self {
        Self {
            username: username.into(),
            role,
            nickname: nickname.into(),
            password_hash: Secret::new(password_hash),
        }
    }

    pub fn password_hash_is_set(&self) -> bool {
        !self.password_hash.expose().is_empty()
    }

    pub fn validate_password_hash(&self) -> GuardResult<()> {
        PasswordHash::new(self.password_hash.expose()).map_err(|error| {
            GuardError::InvalidConfig(format!("invalid Argon2 password hash: {error}"))
        })?;
        Ok(())
    }

    pub fn verify_password(&self, password: &str) -> GuardResult<bool> {
        let hash = PasswordHash::new(self.password_hash.expose()).map_err(|error| {
            GuardError::InvalidConfig(format!("invalid Argon2 password hash: {error}"))
        })?;
        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok())
    }
}
