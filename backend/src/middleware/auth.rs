use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Mutex,
    time::{Duration as StdDuration, Instant},
};

use axum::http::HeaderMap;
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::sync::{Semaphore, SemaphorePermit};
use uuid::Uuid;

use crate::{
    config::AppConfig,
    db::models::User,
    error::{AppError, AppResult},
};

const LOGIN_WINDOW: StdDuration = StdDuration::from_secs(60);
const MAX_LOGIN_FAILURES: u32 = 5;
const LOGIN_DELAY_STEP: StdDuration = StdDuration::from_millis(250);
const LOGIN_DELAY_MAX: StdDuration = StdDuration::from_secs(2);
const LOGIN_HASH_PERMITS: usize = 4;
const LOGIN_HASH_QUEUE_TIMEOUT: StdDuration = StdDuration::from_millis(500);
const MAX_LOGIN_ATTEMPT_KEYS: usize = 10_000;
const MAX_LOGIN_USERNAME_LEN: usize = 100;
const REVOKED_TOKEN_PRUNE_INTERVAL_SECONDS: usize = 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: Uuid,
    pub username: String,
    pub role: String,
    #[serde(default)]
    pub token_version: i64,
    pub exp: usize,
}

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: Uuid,
    pub username: String,
    pub role: String,
    pub token_version: i64,
}

#[derive(Debug)]
struct LoginAttempt {
    failures: u32,
    window_started: Instant,
}

#[derive(Debug)]
pub struct LoginThrottleKeys {
    account_key: String,
}

#[derive(Debug)]
pub struct AuthState {
    login_attempts: tokio::sync::Mutex<HashMap<String, LoginAttempt>>,
    login_hash_permits: Semaphore,
    revoked_tokens: Mutex<HashMap<String, usize>>,
    last_revoked_token_prune: Mutex<usize>,
}

impl Default for AuthState {
    fn default() -> Self {
        Self {
            login_attempts: tokio::sync::Mutex::new(HashMap::new()),
            login_hash_permits: Semaphore::new(LOGIN_HASH_PERMITS),
            revoked_tokens: Mutex::new(HashMap::new()),
            last_revoked_token_prune: Mutex::new(0),
        }
    }
}

impl AuthState {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn check_login_allowed(
        &self,
        _peer_addr: SocketAddr,
        username: &str,
    ) -> AppResult<LoginThrottleKeys> {
        let username = normalize_login_username(username)?;
        let account_key = format!("account:{username}");
        self.check_throttle_key(&account_key).await?;

        Ok(LoginThrottleKeys { account_key })
    }

    pub async fn record_login_failure(&self, keys: &LoginThrottleKeys) {
        self.record_throttle_failure(&keys.account_key).await;
    }

    pub async fn clear_login_failures(&self, keys: &LoginThrottleKeys) {
        let mut attempts = self.login_attempts.lock().await;
        attempts.remove(&keys.account_key);
    }

    pub async fn acquire_login_hash_permit(&self) -> AppResult<SemaphorePermit<'_>> {
        match tokio::time::timeout(LOGIN_HASH_QUEUE_TIMEOUT, self.login_hash_permits.acquire())
            .await
        {
            Ok(Ok(permit)) => Ok(permit),
            Ok(Err(err)) => Err(AppError::Internal(err.into())),
            Err(_) => Err(AppError::TooManyRequests),
        }
    }

    pub fn authorize(&self, headers: &HeaderMap, config: &AppConfig) -> AppResult<AuthUser> {
        let token = bearer_token(headers)?;
        self.authorize_token(token, config)
    }

    fn authorize_token(&self, token: &str, config: &AppConfig) -> AppResult<AuthUser> {
        self.reject_revoked_token(token)?;
        let token_data = decode_token(token, config)?;

        Ok(AuthUser {
            id: token_data.claims.sub,
            username: token_data.claims.username,
            role: token_data.claims.role,
            token_version: token_data.claims.token_version,
        })
    }

    /// Validates the token and reloads the user so deleted accounts and role changes take effect
    /// immediately instead of trusting stale JWT claims until token expiry.
    pub async fn authorize_active(
        &self,
        headers: &HeaderMap,
        config: &AppConfig,
        pool: &PgPool,
    ) -> AppResult<AuthUser> {
        let token = bearer_token(headers)?;
        self.authorize_token_active(token, config, pool).await
    }

    pub async fn authorize_token_active(
        &self,
        token: &str,
        config: &AppConfig,
        pool: &PgPool,
    ) -> AppResult<AuthUser> {
        let token_user = self.authorize_token(token, config)?;
        let active_user = sqlx::query_as::<_, (Uuid, String, String, i64)>(
            "SELECT id, username, role, token_version FROM users WHERE id = $1",
        )
        .bind(token_user.id)
        .fetch_optional(pool)
        .await?;
        let Some((id, username, role, token_version)) = active_user else {
            return Err(AppError::Unauthorized);
        };
        if token_user.token_version != token_version {
            return Err(AppError::Unauthorized);
        }
        Ok(AuthUser {
            id,
            username,
            role,
            token_version,
        })
    }

    pub fn revoke_token(&self, headers: &HeaderMap, config: &AppConfig) -> AppResult<()> {
        let token = bearer_token(headers)?;
        self.reject_revoked_token(token)?;
        let token_data = decode_token(token, config)?;
        let mut revoked_tokens = self
            .revoked_tokens
            .lock()
            .map_err(|_| AppError::Internal(anyhow::anyhow!("revoked token lock poisoned")))?;
        revoked_tokens.insert(token.to_string(), token_data.claims.exp);
        Ok(())
    }

    async fn check_throttle_key(&self, key: &str) -> AppResult<()> {
        let now = Instant::now();
        let mut attempts = self.login_attempts.lock().await;
        prune_login_attempts(&mut attempts, now);

        let delay = attempts
            .get(key)
            .map(login_failure_delay)
            .unwrap_or_default();
        drop(attempts);
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        Ok(())
    }

    async fn record_throttle_failure(&self, key: &str) {
        let now = Instant::now();
        let mut attempts = self.login_attempts.lock().await;
        prune_login_attempts(&mut attempts, now);

        if attempts.len() >= MAX_LOGIN_ATTEMPT_KEYS && !attempts.contains_key(key) {
            if let Some(oldest_key) = attempts.keys().next().cloned() {
                attempts.remove(&oldest_key);
            }
        }

        let attempt = attempts.entry(key.to_string()).or_insert(LoginAttempt {
            failures: 0,
            window_started: now,
        });

        if now.duration_since(attempt.window_started) > LOGIN_WINDOW {
            attempt.failures = 0;
            attempt.window_started = now;
        }

        attempt.failures += 1;
    }

    fn reject_revoked_token(&self, token: &str) -> AppResult<()> {
        self.prune_revoked_tokens_if_due()?;
        let revoked_tokens = self
            .revoked_tokens
            .lock()
            .map_err(|_| AppError::Internal(anyhow::anyhow!("revoked token lock poisoned")))?;
        if revoked_tokens.contains_key(token) {
            Err(AppError::Unauthorized)
        } else {
            Ok(())
        }
    }

    fn prune_revoked_tokens_if_due(&self) -> AppResult<()> {
        let now = Utc::now().timestamp() as usize;
        let mut last_prune = self.last_revoked_token_prune.lock().map_err(|_| {
            AppError::Internal(anyhow::anyhow!("revoked token prune lock poisoned"))
        })?;
        if now.saturating_sub(*last_prune) < REVOKED_TOKEN_PRUNE_INTERVAL_SECONDS {
            return Ok(());
        }
        *last_prune = now;
        drop(last_prune);

        let mut revoked_tokens = self
            .revoked_tokens
            .lock()
            .map_err(|_| AppError::Internal(anyhow::anyhow!("revoked token lock poisoned")))?;
        revoked_tokens.retain(|_, exp| *exp > now);
        Ok(())
    }
}

pub fn create_token(user: &User, config: &AppConfig) -> AppResult<(String, usize)> {
    let expires_at = Utc::now() + Duration::seconds(config.jwt_ttl_seconds);
    let exp = expires_at.timestamp() as usize;
    let claims = Claims {
        sub: user.id,
        username: user.username.clone(),
        role: user.role.clone(),
        token_version: user.token_version,
        exp,
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(config.jwt_secret.as_bytes()),
    )
    .map_err(|err| AppError::Internal(err.into()))?;

    Ok((token, exp))
}

fn bearer_token(headers: &HeaderMap) -> AppResult<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|header| header.strip_prefix("Bearer "))
        .ok_or(AppError::Unauthorized)
}

fn decode_token(token: &str, config: &AppConfig) -> AppResult<jsonwebtoken::TokenData<Claims>> {
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(config.jwt_secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| AppError::Unauthorized)
}

fn normalize_login_username(username: &str) -> AppResult<String> {
    let username = username.trim();
    if username.is_empty() || username.len() > MAX_LOGIN_USERNAME_LEN {
        return Err(AppError::Unauthorized);
    }
    Ok(username.to_string())
}

fn prune_login_attempts(attempts: &mut HashMap<String, LoginAttempt>, now: Instant) {
    attempts.retain(|_, attempt| now.duration_since(attempt.window_started) <= LOGIN_WINDOW);
}

fn login_failure_delay(attempt: &LoginAttempt) -> StdDuration {
    let steps = attempt
        .failures
        .saturating_sub(MAX_LOGIN_FAILURES.saturating_sub(1));
    LOGIN_DELAY_STEP.saturating_mul(steps).min(LOGIN_DELAY_MAX)
}

pub fn require_admin(user: &AuthUser) -> AppResult<()> {
    if user.role == "admin" {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

#[cfg(test)]
mod tests;
