use axum::{
    extract::{Path, State},
    http::HeaderMap,
    routing::{delete, get},
    Json, Router,
};
use bcrypt::{hash, DEFAULT_COST};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::{
    db::models::User,
    error::{AppError, AppResult},
    middleware::auth::require_admin,
    AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_users).post(create_user))
        .route("/:id", delete(delete_user))
}

#[derive(Debug, Deserialize)]
pub(super) struct CreateUserRequest {
    username: String,
    password: String,
    role: Option<String>,
}

pub(super) async fn list_users(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<serde_json::Value>> {
    let auth = state
        .auth
        .authorize_active(&headers, &state.config, &state.db)
        .await?;
    require_admin(&auth)?;

    let users = sqlx::query_as::<_, User>(
        "SELECT id, username, password_hash, role, token_version, created_at, updated_at FROM users ORDER BY created_at DESC",
    )
    .fetch_all(&state.db)
    .await?;

    Ok(Json(json!({ "data": users })))
}

pub(super) async fn create_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<CreateUserRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let auth = state
        .auth
        .authorize_active(&headers, &state.config, &state.db)
        .await?;
    require_admin(&auth)?;
    validate_password(&payload.password)?;
    let username = normalize_username(&payload.username)?;

    let role = payload.role.unwrap_or_else(|| "admin".into());
    if role != "admin" {
        return Err(AppError::Validation("role must be admin".into()));
    }

    let password = payload.password;
    let password_hash = tokio::task::spawn_blocking(move || hash(password, DEFAULT_COST))
        .await
        .map_err(|err| AppError::Internal(err.into()))?
        .map_err(|err| AppError::Internal(err.into()))?;
    let user = sqlx::query_as::<_, User>(
        r#"
        INSERT INTO users (id, username, password_hash, role)
        VALUES ($1, $2, $3, $4)
        RETURNING id, username, password_hash, role, token_version, created_at, updated_at
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(username)
    .bind(password_hash)
    .bind(role)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(json!({ "data": user })))
}

async fn delete_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let auth = state
        .auth
        .authorize_active(&headers, &state.config, &state.db)
        .await?;
    require_admin(&auth)?;
    if auth.id == id {
        return Err(AppError::Validation("cannot delete current user".into()));
    }

    let result = sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }

    Ok(Json(json!({ "data": { "deleted": true } })))
}

fn validate_password(password: &str) -> AppResult<()> {
    let length = password.len();
    if length < 12 {
        return Err(AppError::Validation(
            "password must be at least 12 bytes".into(),
        ));
    }
    if length > 72 {
        return Err(AppError::Validation(
            "password must be at most 72 bytes".into(),
        ));
    }
    Ok(())
}

fn normalize_username(username: &str) -> AppResult<String> {
    let username = username.trim();
    if username.is_empty() {
        return Err(AppError::Validation("username is required".into()));
    }
    if username.len() > 100 {
        return Err(AppError::Validation(
            "username must be at most 100 bytes".into(),
        ));
    }
    if username.chars().any(char::is_control) {
        return Err(AppError::Validation(
            "username must not contain control characters".into(),
        ));
    }
    Ok(username.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_password_bcrypt_limits() {
        assert!(validate_password("short").is_err());
        assert!(validate_password(&"x".repeat(12)).is_ok());
        assert!(validate_password(&"x".repeat(73)).is_err());
    }

    #[test]
    fn normalizes_and_validates_usernames() {
        assert_eq!(normalize_username("  admin  ").unwrap(), "admin");
        assert!(normalize_username("   ").is_err());
        assert!(normalize_username("bad\nname").is_err());
        assert!(normalize_username(&"x".repeat(101)).is_err());
    }
}
