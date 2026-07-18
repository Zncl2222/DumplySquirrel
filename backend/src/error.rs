use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("too many requests")]
    TooManyRequests,
    #[error("not found")]
    NotFound,
    #[error("backup file is missing")]
    FileMissing,
    #[error("validation error: {0}")]
    Validation(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("backup timed out")]
    BackupTimeout,
    #[error("backup output is sealed but its database commit is pending recovery: {0}")]
    BackupCommitPending(String),
    #[error("internal error")]
    Internal(#[from] anyhow::Error),
}

impl From<sqlx::Error> for AppError {
    fn from(value: sqlx::Error) -> Self {
        match value {
            sqlx::Error::RowNotFound => Self::NotFound,
            sqlx::Error::Database(err) if err.code().as_deref() == Some("23505") => {
                Self::Conflict("unique constraint violation".into())
            }
            other => Self::Internal(other.into()),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "UNAUTHORIZED", self.to_string()),
            Self::Forbidden => (StatusCode::FORBIDDEN, "FORBIDDEN", self.to_string()),
            Self::TooManyRequests => (
                StatusCode::TOO_MANY_REQUESTS,
                "TOO_MANY_REQUESTS",
                self.to_string(),
            ),
            Self::NotFound => (StatusCode::NOT_FOUND, "NOT_FOUND", self.to_string()),
            Self::FileMissing => (StatusCode::NOT_FOUND, "FILE_MISSING", self.to_string()),
            Self::Validation(_) => (
                StatusCode::BAD_REQUEST,
                "VALIDATION_ERROR",
                self.to_string(),
            ),
            Self::Conflict(_) => (StatusCode::CONFLICT, "CONFLICT", self.to_string()),
            Self::BackupTimeout => (
                StatusCode::REQUEST_TIMEOUT,
                "BACKUP_TIMEOUT",
                self.to_string(),
            ),
            Self::Internal(_) | Self::BackupCommitPending(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                "internal error".to_string(),
            ),
        };

        tracing::error!(error = ?self, "request failed");
        (
            status,
            Json(json!({ "error": { "code": code, "message": message } })),
        )
            .into_response()
    }
}
