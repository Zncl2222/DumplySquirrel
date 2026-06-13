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
    middleware::auth::{authorize, require_admin},
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
    let auth = authorize(&headers, &state.config)?;
    require_admin(&auth)?;

    let users = sqlx::query_as::<_, User>(
        "SELECT id, username, password_hash, role, created_at, updated_at FROM users ORDER BY created_at DESC",
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
    let auth = authorize(&headers, &state.config)?;
    require_admin(&auth)?;
    validate_password(&payload.password)?;

    let role = payload.role.unwrap_or_else(|| "admin".into());
    if role != "admin" {
        return Err(AppError::Validation("role must be admin".into()));
    }

    let password_hash =
        hash(payload.password, DEFAULT_COST).map_err(|err| AppError::Internal(err.into()))?;
    let user = sqlx::query_as::<_, User>(
        r#"
        INSERT INTO users (id, username, password_hash, role)
        VALUES ($1, $2, $3, $4)
        RETURNING id, username, password_hash, role, created_at, updated_at
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(payload.username)
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
    let auth = authorize(&headers, &state.config)?;
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
    if password.len() < 12 {
        return Err(AppError::Validation(
            "password must be at least 12 characters".into(),
        ));
    }
    Ok(())
}
