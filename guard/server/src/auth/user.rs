use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use uuid::Uuid;

use crate::auth::{Role, Secret};
use crate::core::{GuardError, GuardResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserAccess {
    pub enabled: bool,
    pub expires_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserProfile {
    pub username: String,
    pub role: Role,
    pub nickname: String,
    pub enabled: bool,
    pub expires_at_ms: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone)]
pub struct UserAccount {
    pub username: String,
    pub role: Role,
    pub nickname: String,
    pub expires_at_ms: Option<i64>,
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
        Self::with_nickname_and_expiration(username, role, nickname, password_hash, None)
    }

    pub fn with_nickname_and_expiration(
        username: impl Into<String>,
        role: Role,
        nickname: impl Into<String>,
        password_hash: impl Into<String>,
        expires_at_ms: Option<i64>,
    ) -> Self {
        Self {
            username: username.into(),
            role,
            nickname: nickname.into(),
            expires_at_ms,
            password_hash: Secret::new(password_hash),
        }
    }

    pub fn is_expired_at(&self, now_ms: u64) -> bool {
        let now_ms = i64::try_from(now_ms).unwrap_or(i64::MAX);
        self.expires_at_ms
            .is_some_and(|expires_at_ms| expires_at_ms <= now_ms)
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

#[must_use]
pub fn password_is_present(password: &str) -> bool {
    !password.is_empty()
}

pub fn hash_password(password: &str) -> GuardResult<String> {
    if !password_is_present(password) {
        return Err(GuardError::InvalidConfig(
            "password is required".to_string(),
        ));
    }
    let salt = SaltString::encode_b64(Uuid::new_v4().as_bytes())
        .map_err(|error| GuardError::InvalidConfig(format!("invalid password salt: {error}")))?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| GuardError::InvalidConfig(format!("password hash failed: {error}")))
}
