use axum::{
    extract::{ConnectInfo, State},
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use bcrypt::{hash, verify, DEFAULT_COST};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::SocketAddr;
use uuid::Uuid;

use crate::{
    db::models::User,
    error::{AppError, AppResult},
    middleware::auth::create_token,
    AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/me", get(me))
}

#[derive(Debug, Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Debug, Serialize)]
struct UserResponse {
    id: Uuid,
    username: String,
    role: String,
}

impl From<User> for UserResponse {
    fn from(user: User) -> Self {
        Self {
            id: user.id,
            username: user.username,
            role: user.role,
        }
    }
}

async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    Json(payload): Json<LoginRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let throttle_keys = state
        .auth
        .check_login_allowed(peer_addr, &payload.username)
        .await?;
    let _hash_permit = state.auth.acquire_login_hash_permit().await?;
    if payload.password.len() > 72 {
        consume_password_hash_cost(payload.password).await?;
        state.auth.record_login_failure(&throttle_keys).await;
        return Err(AppError::Unauthorized);
    }
    let username = payload.username.trim().to_string();

    let user = sqlx::query_as::<_, User>(
        r#"
        SELECT id, username, password_hash, role, token_version, created_at, updated_at
        FROM users WHERE username = $1
        "#,
    )
    .bind(username)
    .fetch_optional(&state.db)
    .await?;
    let Some(user) = user else {
        consume_password_hash_cost(payload.password).await?;
        state.auth.record_login_failure(&throttle_keys).await;
        return Err(AppError::Unauthorized);
    };

    let password_hash = user.password_hash.clone();
    let valid_password =
        tokio::task::spawn_blocking(move || verify(payload.password, &password_hash))
            .await
            .map_err(|err| AppError::Internal(err.into()))?
            .map_err(|err| AppError::Internal(err.into()))?;
    if !valid_password {
        state.auth.record_login_failure(&throttle_keys).await;
        return Err(AppError::Unauthorized);
    }

    state.auth.clear_login_failures(&throttle_keys).await;
    let (token, expires_at) = create_token(&user, &state.config)?;
    Ok(Json(json!({
        "data": {
            "token": token,
            "expires_at": expires_at,
            "user": UserResponse::from(user)
        }
    })))
}

async fn consume_password_hash_cost(password: String) -> AppResult<()> {
    tokio::task::spawn_blocking(move || hash(password, DEFAULT_COST))
        .await
        .map_err(|err| AppError::Internal(err.into()))?
        .map_err(|err| AppError::Internal(err.into()))?;
    Ok(())
}

async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<serde_json::Value>> {
    let auth = state
        .auth
        .authorize_active(&headers, &state.config, &state.db)
        .await?;
    let result = sqlx::query(
        r#"
        UPDATE users
        SET token_version = token_version + 1, updated_at = now()
        WHERE id = $1 AND token_version = $2
        "#,
    )
    .bind(auth.id)
    .bind(auth.token_version)
    .execute(&state.db)
    .await?;
    if result.rows_affected() != 1 {
        return Err(AppError::Unauthorized);
    }
    state.auth.revoke_token(&headers, &state.config)?;
    Ok(Json(json!({
        "data": {
            "message": "logged out",
            "all_sessions_invalidated": true
        }
    })))
}

async fn me(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<serde_json::Value>> {
    let auth = state
        .auth
        .authorize_active(&headers, &state.config, &state.db)
        .await?;
    Ok(Json(json!({
        "data": {
            "id": auth.id,
            "username": auth.username,
            "role": auth.role
        }
    })))
}
