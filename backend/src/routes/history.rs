use axum::{
    body::Body,
    extract::{Form, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use sqlx::{Postgres, QueryBuilder};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use std::path::Path as FsPath;

use crate::{
    db::models::BackupHistory,
    error::{AppError, AppResult},
    services::backup_executor,
    AppState,
};

const MAX_HISTORY_OFFSET: i64 = 100_000;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_history))
        .route("/:id", get(get_history))
        .route("/:id/cancel", post(cancel_history))
        .route(
            "/:id/download",
            get(download_history).post(download_history_form),
        )
}

async fn cancel_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> AppResult<(StatusCode, Json<serde_json::Value>)> {
    state
        .auth
        .authorize_active(&headers, &state.config, &state.db)
        .await?;
    backup_executor::request_backup_cancellation(&state.backup_runtime, id).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "data": {
                "history_id": id,
                "cancellation_requested": true
            }
        })),
    ))
}

#[derive(Debug, Deserialize)]
pub(super) struct HistoryQuery {
    config_id: Option<Uuid>,
    status: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct DownloadForm {
    token: String,
}

pub(super) async fn list_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> AppResult<Json<serde_json::Value>> {
    state
        .auth
        .authorize_active(&headers, &state.config, &state.db)
        .await?;
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(20).clamp(1, 100);
    let offset = history_offset(page, per_page)?;
    let status = history_status_filter(query.status.as_deref())?;
    let limit = per_page + 1;

    let mut statement = QueryBuilder::<Postgres>::new(
        r#"
        SELECT id, config_id, status, file_name, file_size, file_path,
               (status = 'success' AND file_path IS NOT NULL) AS is_downloadable,
               error_message, started_at, completed_at, triggered_by
        FROM backup_history
        WHERE TRUE
        "#,
    );
    if let Some(config_id) = query.config_id {
        statement.push(" AND config_id = ").push_bind(config_id);
    }
    if let Some(status) = status {
        statement.push(" AND status = ").push_bind(status);
    }
    statement
        .push(" ORDER BY started_at DESC, id DESC LIMIT ")
        .push_bind(limit)
        .push(" OFFSET ")
        .push_bind(offset);

    let mut rows = statement
        .build_query_as::<BackupHistory>()
        .fetch_all(&state.db)
        .await?;
    let has_next = rows.len() > per_page as usize;
    if has_next {
        rows.truncate(per_page as usize);
    }

    Ok(Json(json!({
        "data": rows,
        "pagination": { "page": page, "per_page": per_page, "has_next": has_next }
    })))
}

fn history_offset(page: i64, per_page: i64) -> AppResult<i64> {
    let offset = page
        .checked_sub(1)
        .and_then(|page_index| page_index.checked_mul(per_page))
        .ok_or_else(|| AppError::Validation("page is too large".into()))?;
    if offset > MAX_HISTORY_OFFSET {
        return Err(AppError::Validation(format!(
            "page exceeds the maximum history offset of {MAX_HISTORY_OFFSET}; narrow the query with config_id or status"
        )));
    }
    Ok(offset)
}

fn history_status_filter(value: Option<&str>) -> AppResult<Option<String>> {
    let status = value
        .map(str::trim)
        .filter(|status| !status.is_empty())
        .map(str::to_ascii_lowercase);
    if status.as_deref().is_some_and(|status| {
        !matches!(
            status,
            "running" | "success" | "failed" | "timeout" | "cancelled"
        )
    }) {
        return Err(AppError::Validation("invalid backup status filter".into()));
    }
    Ok(status)
}

async fn get_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    state
        .auth
        .authorize_active(&headers, &state.config, &state.db)
        .await?;
    let row = sqlx::query_as::<_, BackupHistory>(
        r#"
        SELECT id, config_id, status, file_name, file_size, file_path,
               (status = 'success' AND file_path IS NOT NULL) AS is_downloadable,
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
    state
        .auth
        .authorize_active(&headers, &state.config, &state.db)
        .await?;
    build_download_response(&state, id).await
}

async fn download_history_form(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Form(form): Form<DownloadForm>,
) -> AppResult<Response> {
    state
        .auth
        .authorize_token_active(&form.token, &state.config, &state.db)
        .await?;
    build_download_response(&state, id).await
}

async fn build_download_response(state: &AppState, id: Uuid) -> AppResult<Response> {
    let row = sqlx::query_as::<_, BackupHistory>(
        r#"
        SELECT id, config_id, status, file_name, file_size, file_path,
               (status = 'success' AND file_path IS NOT NULL) AS is_downloadable,
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
    let file = open_managed_backup_file(&state.config.backup_dir, &requested_path).await?;
    let metadata = file.metadata().await.map_err(|_| AppError::FileMissing)?;
    let file_name = row.file_name.unwrap_or_else(|| {
        requested_path
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
        HeaderValue::from_static("application/octet-stream"),
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

/// Opens only a direct regular child of the managed backup directory.
///
/// On Unix, the directory is opened first and the file is opened relative to that stable handle.
/// `O_NOFOLLOW` closes the time-of-check/time-of-use window where another process with access to
/// the backup mount could replace a validated file with a symlink before it was opened.
async fn open_managed_backup_file(
    backup_dir: &FsPath,
    requested_path: &FsPath,
) -> AppResult<tokio::fs::File> {
    let canonical_backup_dir = tokio::fs::canonicalize(backup_dir)
        .await
        .map_err(|_| AppError::FileMissing)?;
    let requested_parent = requested_path.parent().ok_or(AppError::NotFound)?;
    let canonical_parent = tokio::fs::canonicalize(requested_parent)
        .await
        .map_err(|_| AppError::FileMissing)?;
    if canonical_parent != canonical_backup_dir {
        tracing::warn!(path = %requested_path.display(), "blocked backup download outside backup dir");
        return Err(AppError::NotFound);
    }
    let file_name = requested_path
        .file_name()
        .ok_or(AppError::NotFound)?
        .to_os_string();

    #[cfg(unix)]
    {
        let std_file = tokio::task::spawn_blocking(move || {
            use std::{
                ffi::CString,
                io,
                os::{
                    fd::{AsRawFd, FromRawFd},
                    unix::ffi::OsStrExt,
                },
            };

            let directory = std::fs::File::open(canonical_backup_dir)?;
            let file_name = CString::new(file_name.as_bytes()).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "backup file name contains NUL")
            })?;
            // O_NONBLOCK prevents an attacker-controlled FIFO from blocking the worker before
            // metadata can reject it. It has no effect on regular-file reads.
            let fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    file_name.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let file = unsafe { std::fs::File::from_raw_fd(fd) };
            if !file.metadata()?.file_type().is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "managed backup path is not a regular file",
                ));
            }
            Ok::<_, io::Error>(file)
        })
        .await
        .map_err(|err| AppError::Internal(err.into()))?
        .map_err(|err| {
            tracing::warn!(path = %requested_path.display(), error = ?err, "blocked or missing backup download");
            AppError::FileMissing
        })?;
        Ok(tokio::fs::File::from_std(std_file))
    }

    #[cfg(not(unix))]
    {
        let metadata = tokio::fs::symlink_metadata(requested_path)
            .await
            .map_err(|_| AppError::FileMissing)?;
        if !metadata.file_type().is_file() {
            return Err(AppError::FileMissing);
        }
        tokio::fs::File::open(requested_path)
            .await
            .map_err(|_| AppError::FileMissing)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[test]
    fn history_filters_reject_unbounded_or_unknown_queries() {
        assert_eq!(history_offset(1, 20).unwrap(), 0);
        assert!(history_offset(i64::MAX, 100).is_err());
        assert!(history_offset((MAX_HISTORY_OFFSET / 100) + 2, 100).is_err());
        assert_eq!(
            history_status_filter(Some(" SUCCESS ")).unwrap(),
            Some("success".into())
        );
        assert_eq!(history_status_filter(Some(" ")).unwrap(), None);
        assert!(history_status_filter(Some("unknown")).is_err());
    }

    #[tokio::test]
    async fn managed_download_opens_a_direct_regular_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("backup.sql");
        std::fs::write(&path, b"backup contents").unwrap();

        let mut file = open_managed_backup_file(directory.path(), &path)
            .await
            .unwrap();
        let mut contents = Vec::new();
        file.read_to_end(&mut contents).await.unwrap();
        assert_eq!(contents, b"backup contents");
    }

    #[tokio::test]
    async fn managed_download_rejects_an_outside_file() {
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();

        assert!(open_managed_backup_file(directory.path(), outside.path())
            .await
            .is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn managed_download_never_follows_a_final_symlink() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(outside.path(), b"sensitive contents").unwrap();
        let link = directory.path().join("backup.sql");
        symlink(outside.path(), &link).unwrap();

        assert!(open_managed_backup_file(directory.path(), &link)
            .await
            .is_err());
        assert_eq!(
            std::fs::read(outside.path()).unwrap(),
            b"sensitive contents"
        );
    }
}
