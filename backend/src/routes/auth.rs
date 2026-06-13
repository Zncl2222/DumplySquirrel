use axum::{
    extract::State,
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use bcrypt::verify;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    db::models::User,
    error::{AppError, AppResult},
    middleware::auth::{authorize, create_token},
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
    Json(payload): Json<LoginRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let user = sqlx::query_as::<_, User>(
        r#"
        SELECT id, username, password_hash, role, created_at, updated_at
        FROM users WHERE username = $1
        "#,
    )
    .bind(payload.username)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::Unauthorized)?;

    let valid_password = verify(payload.password, &user.password_hash)
        .map_err(|err| AppError::Internal(err.into()))?;
    if !valid_password {
        return Err(AppError::Unauthorized);
    }

    let (token, expires_at) = create_token(&user, &state.config)?;
    Ok(Json(json!({
        "data": {
            "token": token,
            "expires_at": expires_at,
            "user": UserResponse::from(user)
        }
    })))
}

async fn logout() -> Json<serde_json::Value> {
    Json(json!({ "data": { "message": "client-side logout" } }))
}

async fn me(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<serde_json::Value>> {
    let auth = authorize(&headers, &state.config)?;
    Ok(Json(json!({
        "data": {
            "id": auth.id,
            "username": auth.username,
            "role": auth.role
        }
    })))
}
