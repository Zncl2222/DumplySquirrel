use std::collections::HashMap;

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
    db_url: Option<String>,
    cron_schedule: Option<String>,
    retention_days: Option<i32>,
    timeout_seconds: Option<i32>,
    max_backups: Option<i32>,
    is_enabled: Option<bool>,
    #[serde(default)]
    email_to: Vec<String>,
    #[serde(default)]
    email_cc: Vec<String>,
    email_notify_on: Option<String>,
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
    email_to: Vec<String>,
    email_cc: Vec<String>,
    email_notify_on: String,
    created_by: Option<Uuid>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    last_run_status: Option<String>,
    last_run_at: Option<chrono::DateTime<chrono::Utc>>,
    last_success_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, sqlx::FromRow)]
struct BackupConfigRunState {
    config_id: Uuid,
    last_run_status: Option<String>,
    last_run_at: Option<chrono::DateTime<chrono::Utc>>,
    last_success_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub(super) async fn list_configs(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<serde_json::Value>> {
    state
        .auth
        .authorize_active(&headers, &state.config, &state.db)
        .await?;
    let configs = sqlx::query_as::<_, BackupConfig>(
        r#"
        SELECT id, name, db_type, db_version, db_url_encrypted, db_url_nonce, cron_schedule,
               is_enabled, retention_days, timeout_seconds, max_backups,
               email_to, email_cc, email_notify_on,
               created_by, created_at, updated_at
        FROM backup_configs
        ORDER BY created_at DESC
        "#,
    )
    .fetch_all(&state.db)
    .await?;

    let run_states = load_config_run_states(&state.db).await?;
    let data = configs
        .into_iter()
        .map(|config| {
            let run_state = run_states.get(&config.id);
            config_response(config, &state.config.database_encryption_key, run_state)
        })
        .collect::<AppResult<Vec<_>>>()?;
    Ok(Json(json!({ "data": data })))
}

pub(super) async fn create_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<BackupConfigRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let auth = state
        .auth
        .authorize_active(&headers, &state.config, &state.db)
        .await?;
    validate_payload(&payload)?;
    let db_url = payload
        .db_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::Validation("db_url is required".into()))?;
    backup_executor::validate_database_url(&payload.db_type, db_url)?;

    let mut db_version = normalize_db_version(payload.db_version);
    if db_version.is_none() {
        let detected = backup_executor::detect_db_version(&payload.db_type, db_url, 30).await?;
        db_version = Some(detected);
    }

    let (ciphertext, nonce) =
        crypto::encrypt_string(db_url, &state.config.database_encryption_key)?;

    let config = sqlx::query_as::<_, BackupConfig>(
        r#"
        INSERT INTO backup_configs
            (id, name, db_type, db_version, db_url_encrypted, db_url_nonce, cron_schedule,
             is_enabled, retention_days, timeout_seconds, max_backups, email_to, email_cc,
             email_notify_on, created_by)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, COALESCE($9, 30), COALESCE($10, 3600), $11, $12, $13, $14, $15)
        RETURNING id, name, db_type, db_version, db_url_encrypted, db_url_nonce, cron_schedule,
                  is_enabled, retention_days, timeout_seconds, max_backups,
                  email_to, email_cc, email_notify_on,
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
    .bind(normalize_email_list(&payload.email_to))
    .bind(normalize_email_list(&payload.email_cc))
    .bind(normalize_notify_on(payload.email_notify_on.as_deref()))
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;

    state.scheduler.refresh_config(config.id).await?;

    Ok(Json(json!({
        "data": config_response(config, &state.config.database_encryption_key, None)?
    })))
}

async fn update_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(payload): Json<BackupConfigRequest>,
) -> AppResult<Json<serde_json::Value>> {
    state
        .auth
        .authorize_active(&headers, &state.config, &state.db)
        .await?;
    validate_payload(&payload)?;

    let existing = sqlx::query_as::<_, BackupConfig>(
        r#"
        SELECT id, name, db_type, db_version, db_url_encrypted, db_url_nonce, cron_schedule,
               is_enabled, retention_days, timeout_seconds, max_backups,
               email_to, email_cc, email_notify_on,
               created_by, created_at, updated_at
        FROM backup_configs
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;

    let db_url = payload
        .db_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let mut db_version = normalize_db_version(payload.db_version.clone());
    let (ciphertext, nonce) = if let Some(db_url) = db_url {
        backup_executor::validate_database_url(&payload.db_type, db_url)?;
        if db_version.is_none() {
            let detected = backup_executor::detect_db_version(&payload.db_type, db_url, 30).await?;
            db_version = Some(detected);
        }
        crypto::encrypt_string(db_url, &state.config.database_encryption_key)?
    } else {
        if payload.db_type != existing.db_type {
            return Err(AppError::Validation(
                "db_url is required when changing db_type".into(),
            ));
        }
        (
            existing.db_url_encrypted.clone(),
            existing.db_url_nonce.clone(),
        )
    };

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
            email_to = $12,
            email_cc = $13,
            email_notify_on = $14,
            updated_at = now()
        WHERE id = $1
        RETURNING id, name, db_type, db_version, db_url_encrypted, db_url_nonce, cron_schedule,
                  is_enabled, retention_days, timeout_seconds, max_backups,
                  email_to, email_cc, email_notify_on,
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
    .bind(normalize_email_list(&payload.email_to))
    .bind(normalize_email_list(&payload.email_cc))
    .bind(normalize_notify_on(payload.email_notify_on.as_deref()))
    .fetch_one(&state.db)
    .await?;

    state.scheduler.refresh_config(config.id).await?;

    Ok(Json(json!({
        "data": config_response(config, &state.config.database_encryption_key, None)?
    })))
}

async fn delete_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    state
        .auth
        .authorize_active(&headers, &state.config, &state.db)
        .await?;
    delete_config_records(&state.db, id).await?;
    if let Err(err) = state.scheduler.remove_config(id).await {
        // The database is authoritative. A stale in-memory job will only observe NotFound,
        // while returning an error here would incorrectly imply that deletion was rolled back.
        tracing::warn!(config_id = %id, error = ?err, "config deleted but scheduler cleanup failed");
    }
    Ok(Json(json!({ "data": { "deleted": true } })))
}

async fn delete_config_records(pool: &sqlx::PgPool, id: Uuid) -> AppResult<()> {
    let mut transaction = pool.begin().await?;
    // Backup preparation takes the same lock through creation of its running history.
    // Once this lock is acquired, the running check and delete are one atomic decision.
    let exists: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM backup_configs WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *transaction)
            .await?;
    if exists.is_none() {
        return Err(AppError::NotFound);
    }

    let running: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM backup_history WHERE config_id = $1 AND status = 'running' LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&mut *transaction)
    .await?;
    if running.is_some() {
        return Err(AppError::Conflict(
            "cannot delete config while backup is running".into(),
        ));
    }

    sqlx::query(
        r#"
        INSERT INTO pending_file_deletions (id, file_path)
        SELECT gen_random_uuid(), file_path
        FROM backup_history
        WHERE config_id = $1
          AND file_path IS NOT NULL
          AND btrim(file_path) <> ''
        ON CONFLICT (file_path) DO NOTHING
        "#,
    )
    .bind(id)
    .execute(&mut *transaction)
    .await?;

    let result = sqlx::query("DELETE FROM backup_configs WHERE id = $1")
        .bind(id)
        .execute(&mut *transaction)
        .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    transaction.commit().await?;
    Ok(())
}

async fn toggle_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(payload): Json<ToggleRequest>,
) -> AppResult<Json<serde_json::Value>> {
    state
        .auth
        .authorize_active(&headers, &state.config, &state.db)
        .await?;
    let config = sqlx::query_as::<_, BackupConfig>(
        r#"
        UPDATE backup_configs SET is_enabled = $2, updated_at = now()
        WHERE id = $1
        RETURNING id, name, db_type, db_version, db_url_encrypted, db_url_nonce, cron_schedule,
                  is_enabled, retention_days, timeout_seconds, max_backups,
                  email_to, email_cc, email_notify_on,
                  created_by, created_at, updated_at
        "#,
    )
    .bind(id)
    .bind(payload.is_enabled)
    .fetch_one(&state.db)
    .await?;
    state.scheduler.refresh_config(config.id).await?;
    Ok(Json(json!({
        "data": config_response(config, &state.config.database_encryption_key, None)?
    })))
}

async fn trigger_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    state
        .auth
        .authorize_active(&headers, &state.config, &state.db)
        .await?;
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
    let notify_on = normalize_notify_on(payload.email_notify_on.as_deref());
    if !matches!(notify_on.as_str(), "never" | "failure" | "always") {
        return Err(AppError::Validation(
            "email_notify_on must be never, failure, or always".into(),
        ));
    }
    validate_email_list(&payload.email_to)?;
    validate_email_list(&payload.email_cc)?;
    if notify_on != "never" && normalize_email_list(&payload.email_to).is_empty() {
        return Err(AppError::Validation(
            "email_to is required when email notifications are enabled".into(),
        ));
    }
    Ok(())
}

fn normalize_notify_on(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("never")
        .to_ascii_lowercase()
}

fn normalize_email_list(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .fold(Vec::new(), |mut acc, value| {
            if !acc.contains(&value) {
                acc.push(value);
            }
            acc
        })
}

fn validate_email_list(values: &[String]) -> AppResult<()> {
    for email in normalize_email_list(values) {
        if !is_valid_email(&email) {
            return Err(AppError::Validation(format!(
                "invalid email address `{email}`"
            )));
        }
    }
    Ok(())
}

fn is_valid_email(value: &str) -> bool {
    if value.contains(char::is_whitespace) || value.contains(['\n', '\r']) {
        return false;
    }
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

async fn load_config_run_states(
    pool: &sqlx::PgPool,
) -> AppResult<HashMap<Uuid, BackupConfigRunState>> {
    let states = sqlx::query_as::<_, BackupConfigRunState>(
        r#"
        SELECT c.id AS config_id,
               latest.status AS last_run_status,
               latest.started_at AS last_run_at,
               (
                   SELECT MAX(success.completed_at)
                   FROM backup_history success
                   WHERE success.config_id = c.id AND success.status = 'success'
               ) AS last_success_at
        FROM backup_configs c
        LEFT JOIN LATERAL (
            SELECT history.status, history.started_at
            FROM backup_history history
            WHERE history.config_id = c.id
            ORDER BY history.started_at DESC
            LIMIT 1
        ) latest ON TRUE
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(states
        .into_iter()
        .map(|state| (state.config_id, state))
        .collect())
}

fn config_response(
    config: BackupConfig,
    encryption_key: &str,
    run_state: Option<&BackupConfigRunState>,
) -> AppResult<BackupConfigResponse> {
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
        email_to: config.email_to,
        email_cc: config.email_cc,
        email_notify_on: config.email_notify_on,
        created_by: config.created_by,
        created_at: config.created_at,
        updated_at: config.updated_at,
        last_run_status: run_state.and_then(|state| state.last_run_status.clone()),
        last_run_at: run_state.and_then(|state| state.last_run_at),
        last_success_at: run_state.and_then(|state| state.last_success_at),
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

#[cfg(test)]
mod tests;
