use axum::{
    extract::{Path, State},
    http::HeaderMap,
    routing::{get, patch, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    db::models::BackupConfig,
    error::{AppError, AppResult},
    middleware::auth::authorize,
    services::{
        backup_executor::{self, BackupTrigger},
        crypto, scheduler,
    },
    AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_configs).post(create_config))
        .route("/:id", put(update_config).delete(delete_config))
        .route("/:id/toggle", patch(toggle_config))
        .route("/:id/trigger", post(trigger_config))
}

#[derive(Debug, Deserialize)]
pub(super) struct BackupConfigRequest {
    name: String,
    db_type: String,
    db_version: Option<String>,
    db_url: String,
    cron_schedule: Option<String>,
    retention_days: Option<i32>,
    timeout_seconds: Option<i32>,
    max_backups: Option<i32>,
    is_enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ToggleRequest {
    is_enabled: bool,
}

#[derive(Debug, Serialize)]
struct BackupConfigResponse {
    id: Uuid,
    name: String,
    db_type: String,
    db_version: Option<String>,
    db_url_masked: String,
    cron_schedule: Option<String>,
    is_enabled: bool,
    retention_days: i32,
    timeout_seconds: i32,
    max_backups: Option<i32>,
    created_by: Option<Uuid>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

pub(super) async fn list_configs(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<serde_json::Value>> {
    authorize(&headers, &state.config)?;
    let configs = sqlx::query_as::<_, BackupConfig>(
        r#"
        SELECT id, name, db_type, db_version, db_url_encrypted, db_url_nonce, cron_schedule,
               is_enabled, retention_days, timeout_seconds, max_backups,
               created_by, created_at, updated_at
        FROM backup_configs
        ORDER BY created_at DESC
        "#,
    )
    .fetch_all(&state.db)
    .await?;

    let data = configs
        .into_iter()
        .map(|config| config_response(config, &state.config.database_encryption_key))
        .collect::<AppResult<Vec<_>>>()?;
    Ok(Json(json!({ "data": data })))
}

pub(super) async fn create_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<BackupConfigRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let auth = authorize(&headers, &state.config)?;
    validate_payload(&payload)?;

    let mut db_version = normalize_db_version(payload.db_version);
    if db_version.is_none() {
        let detected = backup_executor::detect_db_version(
            &payload.db_type,
            &payload.db_url,
            30,
        )
        .await?;
        db_version = Some(detected);
    }

    let (ciphertext, nonce) =
        crypto::encrypt_string(&payload.db_url, &state.config.database_encryption_key)?;

    let config = sqlx::query_as::<_, BackupConfig>(
        r#"
        INSERT INTO backup_configs
            (id, name, db_type, db_version, db_url_encrypted, db_url_nonce, cron_schedule,
             is_enabled, retention_days, timeout_seconds, max_backups, created_by)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, COALESCE($9, 30), COALESCE($10, 3600), $11, $12)
        RETURNING id, name, db_type, db_version, db_url_encrypted, db_url_nonce, cron_schedule,
                  is_enabled, retention_days, timeout_seconds, max_backups,
                  created_by, created_at, updated_at
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(payload.name)
    .bind(payload.db_type)
    .bind(db_version)
    .bind(ciphertext)
    .bind(nonce)
    .bind(payload.cron_schedule)
    .bind(payload.is_enabled.unwrap_or(true))
    .bind(payload.retention_days)
    .bind(payload.timeout_seconds)
    .bind(payload.max_backups)
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;

    state.scheduler.refresh_config(config.id).await?;

    Ok(Json(json!({
        "data": config_response(config, &state.config.database_encryption_key)?
    })))
}

async fn update_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(payload): Json<BackupConfigRequest>,
) -> AppResult<Json<serde_json::Value>> {
    authorize(&headers, &state.config)?;
    validate_payload(&payload)?;

    let mut db_version = normalize_db_version(payload.db_version);
    if db_version.is_none() {
        let detected = backup_executor::detect_db_version(
            &payload.db_type,
            &payload.db_url,
            30,
        )
        .await?;
        db_version = Some(detected);
    }

    let (ciphertext, nonce) =
        crypto::encrypt_string(&payload.db_url, &state.config.database_encryption_key)?;

    let config = sqlx::query_as::<_, BackupConfig>(
        r#"
        UPDATE backup_configs
        SET name = $2,
            db_type = $3,
            db_version = $4,
            db_url_encrypted = $5,
            db_url_nonce = $6,
            cron_schedule = $7,
            is_enabled = $8,
            retention_days = COALESCE($9, retention_days),
            timeout_seconds = COALESCE($10, timeout_seconds),
            max_backups = $11,
            updated_at = now()
        WHERE id = $1
        RETURNING id, name, db_type, db_version, db_url_encrypted, db_url_nonce, cron_schedule,
                  is_enabled, retention_days, timeout_seconds, max_backups,
                  created_by, created_at, updated_at
        "#,
    )
    .bind(id)
    .bind(payload.name)
    .bind(payload.db_type)
    .bind(db_version)
    .bind(ciphertext)
    .bind(nonce)
    .bind(payload.cron_schedule)
    .bind(payload.is_enabled.unwrap_or(true))
    .bind(payload.retention_days)
    .bind(payload.timeout_seconds)
    .bind(payload.max_backups)
    .fetch_one(&state.db)
    .await?;

    state.scheduler.refresh_config(config.id).await?;

    Ok(Json(json!({
        "data": config_response(config, &state.config.database_encryption_key)?
    })))
}

async fn delete_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    authorize(&headers, &state.config)?;
    let running: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM backup_history WHERE config_id = $1 AND status = 'running' LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    if running.is_some() {
        return Err(AppError::Conflict(
            "cannot delete config while backup is running".into(),
        ));
    }

    let backup_files = sqlx::query_scalar::<_, String>(
        "SELECT file_path FROM backup_history WHERE config_id = $1 AND file_path IS NOT NULL",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    let result = sqlx::query("DELETE FROM backup_configs WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    state.scheduler.remove_config(id).await?;
    cleanup_deleted_config_files(&state, backup_files).await;
    Ok(Json(json!({ "data": { "deleted": true } })))
}

async fn toggle_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(payload): Json<ToggleRequest>,
) -> AppResult<Json<serde_json::Value>> {
    authorize(&headers, &state.config)?;
    let config = sqlx::query_as::<_, BackupConfig>(
        r#"
        UPDATE backup_configs SET is_enabled = $2, updated_at = now()
        WHERE id = $1
        RETURNING id, name, db_type, db_version, db_url_encrypted, db_url_nonce, cron_schedule,
                  is_enabled, retention_days, timeout_seconds, max_backups,
                  created_by, created_at, updated_at
        "#,
    )
    .bind(id)
    .bind(payload.is_enabled)
    .fetch_one(&state.db)
    .await?;
    state.scheduler.refresh_config(config.id).await?;
    Ok(Json(json!({
        "data": config_response(config, &state.config.database_encryption_key)?
    })))
}

async fn trigger_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    authorize(&headers, &state.config)?;
    let exists: Option<Uuid> = sqlx::query_scalar("SELECT id FROM backup_configs WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
    if exists.is_none() {
        return Err(AppError::NotFound);
    }

    let history =
        backup_executor::spawn_backup(state.backup_runtime.clone(), id, BackupTrigger::Manual)
            .await?;
    Ok(Json(json!({
        "data": history
    })))
}

fn validate_payload(payload: &BackupConfigRequest) -> AppResult<()> {
    if payload.name.trim().is_empty() {
        return Err(AppError::Validation("name is required".into()));
    }
    if !matches!(payload.db_type.as_str(), "postgres" | "mysql") {
        return Err(AppError::Validation(
            "db_type must be postgres or mysql".into(),
        ));
    }
    validate_db_version(&payload.db_type, payload.db_version.as_deref())?;
    backup_executor::validate_database_url(&payload.db_type, &payload.db_url)?;
    if let Some(schedule) = &payload.cron_schedule {
        scheduler::validate_cron_expression(schedule)?;
    }
    if payload.retention_days.is_some_and(|value| value < 1) {
        return Err(AppError::Validation(
            "retention_days must be positive".into(),
        ));
    }
    if payload.timeout_seconds.is_some_and(|value| value < 1) {
        return Err(AppError::Validation(
            "timeout_seconds must be positive".into(),
        ));
    }
    if payload.max_backups.is_some_and(|value| value < 1) {
        return Err(AppError::Validation("max_backups must be positive".into()));
    }
    Ok(())
}

fn config_response(config: BackupConfig, encryption_key: &str) -> AppResult<BackupConfigResponse> {
    let db_url = crypto::decrypt_string(
        &config.db_url_encrypted,
        &config.db_url_nonce,
        encryption_key,
    )?;
    Ok(BackupConfigResponse {
        id: config.id,
        name: config.name,
        db_type: config.db_type,
        db_version: config.db_version,
        db_url_masked: crypto::mask_database_url(&db_url),
        cron_schedule: config.cron_schedule,
        is_enabled: config.is_enabled,
        retention_days: config.retention_days,
        timeout_seconds: config.timeout_seconds,
        max_backups: config.max_backups,
        created_by: config.created_by,
        created_at: config.created_at,
        updated_at: config.updated_at,
    })
}

fn normalize_db_version(version: Option<String>) -> Option<String> {
    version.and_then(|value| {
        let value = value.trim();
        if value.is_empty() || value.eq_ignore_ascii_case("auto") {
            None
        } else {
            Some(value.to_string())
        }
    })
}

fn validate_db_version(db_type: &str, version: Option<&str>) -> AppResult<()> {
    let Some(version) = version.map(str::trim) else {
        return Ok(());
    };
    if version.is_empty() || version.eq_ignore_ascii_case("auto") {
        return Ok(());
    }

    let valid = match db_type {
        "postgres" => matches!(version, "14" | "15" | "16" | "17" | "18"),
        "mysql" => matches!(version, "8.0" | "8.4" | "9.7"),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(AppError::Validation(match db_type {
            "postgres" => "db_version must be auto, 14, 15, 16, 17, or 18".into(),
            "mysql" => "db_version must be auto, 8.0, 8.4, or 9.7".into(),
            _ => "unsupported db_type".into(),
        }))
    }
}

async fn cleanup_deleted_config_files(state: &AppState, file_paths: Vec<String>) {
    let Ok(backup_dir) = tokio::fs::canonicalize(&state.config.backup_dir).await else {
        tracing::warn!(path = %state.config.backup_dir.display(), "cannot canonicalize backup dir for config file cleanup");
        return;
    };

    for file_path in file_paths {
        let path = std::path::PathBuf::from(&file_path);
        match tokio::fs::canonicalize(&path).await {
            Ok(canonical_path) if canonical_path.starts_with(&backup_dir) => {
                if let Err(err) = tokio::fs::remove_file(&canonical_path).await {
                    if err.kind() != std::io::ErrorKind::NotFound {
                        tracing::warn!(path = %canonical_path.display(), error = ?err, "failed to remove deleted config backup file");
                    }
                }
            }
            Ok(canonical_path) => {
                tracing::warn!(path = %canonical_path.display(), "skipped deleted config file outside backup dir");
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                tracing::warn!(path = %file_path, error = ?err, "failed to canonicalize deleted config backup file");
            }
        }
    }
}
