use axum::http::HeaderMap;
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    config::AppConfig,
    db::models::User,
    error::{AppError, AppResult},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: Uuid,
    pub username: String,
    pub role: String,
    pub exp: usize,
}

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: Uuid,
    pub username: String,
    pub role: String,
}

pub fn create_token(user: &User, config: &AppConfig) -> AppResult<(String, usize)> {
    let expires_at = Utc::now() + Duration::seconds(config.jwt_ttl_seconds);
    let exp = expires_at.timestamp() as usize;
    let claims = Claims {
        sub: user.id,
        username: user.username.clone(),
        role: user.role.clone(),
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

pub fn authorize(headers: &HeaderMap, config: &AppConfig) -> AppResult<AuthUser> {
    let header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or(AppError::Unauthorized)?;
    let token = header
        .strip_prefix("Bearer ")
        .ok_or(AppError::Unauthorized)?;
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(config.jwt_secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| AppError::Unauthorized)?;

    Ok(AuthUser {
        id: token_data.claims.sub,
        username: token_data.claims.username,
        role: token_data.claims.role,
    })
}

pub fn require_admin(user: &AuthUser) -> AppResult<()> {
    if user.role == "admin" {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}
