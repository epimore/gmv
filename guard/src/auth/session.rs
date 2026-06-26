use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::{Mutex, RwLock};
use uuid::Uuid;

use crate::auth::{Role, UserAccount};
use crate::core::{GuardError, GuardResult};

pub const SESSION_COOKIE: &str = "gmv_session";

#[derive(Debug, Clone)]
pub struct SessionPolicy {
    pub allowed_origin: String,
    pub secure_cookie: bool,
    pub session_ttl: Duration,
    pub login_window: Duration,
    pub max_failed_attempts: usize,
}

impl Default for SessionPolicy {
    fn default() -> Self {
        Self {
            allowed_origin: "https://127.0.0.1".to_string(),
            secure_cookie: true,
            session_ttl: Duration::from_secs(8 * 60 * 60),
            login_window: Duration::from_secs(60),
            max_failed_attempts: 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiSession {
    pub username: String,
    pub role: Role,
    pub nickname: String,
    pub csrf_token: String,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone)]
pub struct AuthState {
    users: Arc<RwLock<HashMap<String, UserAccount>>>,
    sessions: Arc<Mutex<HashMap<String, UiSession>>>,
    failed_attempts: Arc<Mutex<HashMap<String, Vec<u64>>>>,
    policy: SessionPolicy,
}

impl AuthState {
    pub fn new(users: impl IntoIterator<Item = UserAccount>, policy: SessionPolicy) -> Self {
        Self {
            users: Arc::new(RwLock::new(
                users
                    .into_iter()
                    .map(|user| (user.username.clone(), user))
                    .collect(),
            )),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            failed_attempts: Arc::new(Mutex::new(HashMap::new())),
            policy,
        }
    }

    pub fn allowed_origin(&self) -> &str {
        &self.policy.allowed_origin
    }

    pub fn authenticate(&self, username: &str, password: &str) -> GuardResult<(String, UiSession)> {
        let now_ms = now_ms()?;
        self.check_rate_limit(username, now_ms)?;
        let user = self.users.read().get(username).cloned();
        let verified = user
            .as_ref()
            .map(|user| user.verify_password(password))
            .transpose()?
            .unwrap_or(false);
        if !verified {
            self.record_failure(username, now_ms);
            return Err(GuardError::InvalidIdentity(
                "invalid username or password".to_string(),
            ));
        }
        self.failed_attempts.lock().remove(username);
        let user = user.expect("verified user must exist");
        let token = Uuid::new_v4().to_string();
        let session = UiSession {
            username: user.username.clone(),
            role: user.role,
            nickname: user.nickname.clone(),
            csrf_token: Uuid::new_v4().to_string(),
            expires_at_ms: now_ms + self.policy.session_ttl.as_millis() as u64,
        };
        self.sessions.lock().insert(token.clone(), session.clone());
        Ok((token, session))
    }

    pub fn upsert_user(&self, user: UserAccount) {
        self.users.write().insert(user.username.clone(), user);
    }

    pub fn remove_user(&self, username: &str) {
        self.users.write().remove(username);
        self.revoke_user_sessions(username);
    }

    pub fn refresh_user_sessions(&self, username: &str, role: Role, nickname: &str) {
        for session in self.sessions.lock().values_mut() {
            if session.username == username {
                session.role = role;
                session.nickname = nickname.to_string();
            }
        }
    }

    pub fn revoke_user_sessions(&self, username: &str) {
        self.sessions
            .lock()
            .retain(|_, session| session.username != username);
    }

    pub fn session(&self, token: &str) -> GuardResult<UiSession> {
        let now_ms = now_ms()?;
        let mut sessions = self.sessions.lock();
        let session = sessions
            .get(token)
            .cloned()
            .ok_or_else(|| GuardError::InvalidIdentity("invalid UI session".to_string()))?;
        if session.expires_at_ms <= now_ms {
            sessions.remove(token);
            return Err(GuardError::InvalidIdentity(
                "expired UI session".to_string(),
            ));
        }
        Ok(session)
    }

    pub fn logout(&self, token: &str) {
        self.sessions.lock().remove(token);
    }

    pub fn require_role(&self, session: &UiSession, required: Role) -> GuardResult<()> {
        if !session.role.allows(required) {
            return Err(GuardError::InvalidIdentity(
                "UI role is not allowed".to_string(),
            ));
        }
        Ok(())
    }

    pub fn verify_csrf(&self, session: &UiSession, candidate: Option<&str>) -> GuardResult<()> {
        if candidate != Some(session.csrf_token.as_str()) {
            return Err(GuardError::InvalidIdentity(
                "invalid CSRF token".to_string(),
            ));
        }
        Ok(())
    }

    pub fn verify_origin(&self, origin: Option<&str>) -> GuardResult<()> {
        if origin != Some(self.policy.allowed_origin.as_str()) {
            return Err(GuardError::InvalidIdentity(
                "request origin is not allowed".to_string(),
            ));
        }
        Ok(())
    }

    pub fn session_cookie(&self, token: &str) -> String {
        let secure = if self.policy.secure_cookie {
            "; Secure"
        } else {
            ""
        };
        format!(
            "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{}",
            self.policy.session_ttl.as_secs(),
            secure
        )
    }

    pub fn clear_cookie(&self) -> String {
        let secure = if self.policy.secure_cookie {
            "; Secure"
        } else {
            ""
        };
        format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{secure}")
    }

    fn check_rate_limit(&self, username: &str, now_ms: u64) -> GuardResult<()> {
        let cutoff = now_ms.saturating_sub(self.policy.login_window.as_millis() as u64);
        let mut attempts = self.failed_attempts.lock();
        let failures = attempts.entry(username.to_string()).or_default();
        failures.retain(|attempt| *attempt >= cutoff);
        if failures.len() >= self.policy.max_failed_attempts {
            return Err(GuardError::Capacity(
                "login rate limit exceeded".to_string(),
            ));
        }
        Ok(())
    }

    fn record_failure(&self, username: &str, now_ms: u64) {
        self.failed_attempts
            .lock()
            .entry(username.to_string())
            .or_default()
            .push(now_ms);
    }
}

pub fn cookie_value(cookie_header: &str, name: &str) -> Option<String> {
    cookie_header.split(';').find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        (key == name).then(|| value.to_string())
    })
}

fn now_ms() -> GuardResult<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .map_err(|error| GuardError::InvalidConfig(format!("system clock before epoch: {error}")))
}
