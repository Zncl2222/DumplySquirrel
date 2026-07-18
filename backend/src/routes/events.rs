use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    db::models::{BackupEvent, BackupHistory},
    error::AppResult,
    AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/running", get(list_running))
        .route("/:history_id", get(list_events))
}

#[derive(Debug, Deserialize)]
struct EventQuery {
    after_sequence: Option<i64>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct BackupConfigSummary {
    id: Uuid,
    name: String,
    db_type: String,
    db_version: Option<String>,
}

#[derive(Debug, Serialize)]
struct RunningBackup {
    history: BackupHistory,
    config: BackupConfigSummary,
    events: Vec<BackupEvent>,
}

#[derive(Debug, sqlx::FromRow)]
struct RunningBackupRow {
    history_id: Uuid,
    config_id: Uuid,
    status: String,
    file_name: Option<String>,
    file_size: Option<i64>,
    file_path: Option<String>,
    error_message: Option<String>,
    started_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
    triggered_by: String,
    config_name: String,
    db_type: String,
    db_version: Option<String>,
}

async fn list_running(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<serde_json::Value>> {
    state
        .auth
        .authorize_active(&headers, &state.config, &state.db)
        .await?;
    let rows = sqlx::query_as::<_, RunningBackupRow>(
        r#"
        SELECT
            h.id AS history_id, h.config_id, h.status, h.file_name, h.file_size, h.file_path,
            h.error_message, h.started_at, h.completed_at, h.triggered_by,
            c.name AS config_name, c.db_type, c.db_version
        FROM backup_history h
        JOIN backup_configs c ON c.id = h.config_id
        WHERE h.status = 'running'
        ORDER BY h.started_at DESC
        "#,
    )
    .fetch_all(&state.db)
    .await?;

    let mut runs = Vec::with_capacity(rows.len());
    for row in rows {
        let history = BackupHistory {
            id: row.history_id,
            config_id: row.config_id,
            status: row.status,
            file_name: row.file_name,
            file_size: row.file_size,
            file_path: row.file_path,
            is_downloadable: false,
            error_message: row.error_message,
            started_at: row.started_at,
            completed_at: row.completed_at,
            triggered_by: row.triggered_by,
        };
        let config = BackupConfigSummary {
            id: row.config_id,
            name: row.config_name,
            db_type: row.db_type,
            db_version: row.db_version,
        };
        let events = events_for_history(&state, history.id, None).await?;
        runs.push(RunningBackup {
            history,
            config,
            events,
        });
    }

    Ok(Json(json!({ "data": runs })))
}

async fn list_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(history_id): Path<Uuid>,
    Query(query): Query<EventQuery>,
) -> AppResult<Json<serde_json::Value>> {
    state
        .auth
        .authorize_active(&headers, &state.config, &state.db)
        .await?;
    let events = events_for_history(&state, history_id, query.after_sequence).await?;
    Ok(Json(json!({ "data": events })))
}

async fn events_for_history(
    state: &AppState,
    history_id: Uuid,
    after_sequence: Option<i64>,
) -> AppResult<Vec<BackupEvent>> {
    Ok(sqlx::query_as::<_, BackupEvent>(
        r#"
        SELECT id, history_id, sequence, stage, level, message, created_at
        FROM backup_events
        WHERE history_id = $1
          AND ($2::bigint IS NULL OR sequence > $2)
        ORDER BY sequence ASC
        "#,
    )
    .bind(history_id)
    .bind(after_sequence)
    .fetch_all(&state.db)
    .await?)
}
