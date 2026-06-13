use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::Response,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::{
    db::models::BackupHistory,
    error::{AppError, AppResult},
    middleware::auth::authorize,
    AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_history))
        .route("/:id", get(get_history))
        .route("/:id/download", get(download_history))
}

#[derive(Debug, Deserialize)]
pub(super) struct HistoryQuery {
    config_id: Option<Uuid>,
    status: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

pub(super) async fn list_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> AppResult<Json<serde_json::Value>> {
    authorize(&headers, &state.config)?;
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;

    let rows = sqlx::query_as::<_, BackupHistory>(
        r#"
        SELECT id, config_id, status, file_name, file_size, file_path,
               error_message, started_at, completed_at, triggered_by
        FROM backup_history
        WHERE ($1::uuid IS NULL OR config_id = $1)
          AND ($2::text IS NULL OR status = $2)
        ORDER BY started_at DESC
        LIMIT $3 OFFSET $4
        "#,
    )
    .bind(query.config_id)
    .bind(query.status)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(json!({
        "data": rows,
        "pagination": { "page": page, "per_page": per_page }
    })))
}

async fn get_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    authorize(&headers, &state.config)?;
    let row = sqlx::query_as::<_, BackupHistory>(
        r#"
        SELECT id, config_id, status, file_name, file_size, file_path,
               error_message, started_at, completed_at, triggered_by
        FROM backup_history WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(json!({ "data": row })))
}

async fn download_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> AppResult<Response> {
    authorize(&headers, &state.config)?;
    let row = sqlx::query_as::<_, BackupHistory>(
        r#"
        SELECT id, config_id, status, file_name, file_size, file_path,
               error_message, started_at, completed_at, triggered_by
        FROM backup_history WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;

    if row.status != "success" {
        return Err(AppError::Validation(
            "only successful backup records can be downloaded".into(),
        ));
    }

    let file_path = row.file_path.ok_or(AppError::FileMissing)?;
    let requested_path = std::path::PathBuf::from(&file_path);
    let backup_dir = tokio::fs::canonicalize(&state.config.backup_dir)
        .await
        .map_err(|_| AppError::FileMissing)?;
    let canonical_path = tokio::fs::canonicalize(&requested_path)
        .await
        .map_err(|_| AppError::FileMissing)?;

    if !canonical_path.starts_with(&backup_dir) {
        tracing::warn!(path = %canonical_path.display(), "blocked backup download outside backup dir");
        return Err(AppError::NotFound);
    }

    let file = tokio::fs::File::open(&canonical_path)
        .await
        .map_err(|_| AppError::FileMissing)?;
    let metadata = file.metadata().await.map_err(|_| AppError::FileMissing)?;
    let file_name = row.file_name.unwrap_or_else(|| {
        canonical_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("backup.sql")
            .to_string()
    });
    let safe_file_name = sanitize_download_file_name(&file_name);
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/sql; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&metadata.len().to_string())
            .map_err(|err| AppError::Internal(err.into()))?,
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{safe_file_name}\""))
            .map_err(|err| AppError::Internal(err.into()))?,
    );

    Ok(response)
}

fn sanitize_download_file_name(file_name: &str) -> String {
    let sanitized = file_name
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' => ch,
            _ => '_',
        })
        .collect::<String>();

    if sanitized.is_empty() {
        "backup.sql".to_string()
    } else {
        sanitized
    }
}
