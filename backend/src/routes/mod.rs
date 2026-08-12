pub mod auth;
pub mod backups;
pub mod dashboard;
pub mod events;
pub mod history;
pub mod users;

use std::path::Path;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::AppState;

pub fn router(_state: AppState) -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/health/live", get(liveness))
        .route("/health/ready", get(readiness))
        .nest("/auth", auth::router())
        .nest("/users", users::router())
        .nest("/backup-configs", backups::router())
        .nest("/backup-events", events::router())
        .nest("/backup-history", history::router())
        .nest("/dashboard", dashboard::router())
}

async fn health(axum::extract::State(state): axum::extract::State<AppState>) -> Response {
    readiness_response(&state).await
}

async fn liveness() -> Json<serde_json::Value> {
    Json(json!({
        "data": {
            "status": "alive",
            "service": "dumply-backend",
            "version": env!("CARGO_PKG_VERSION")
        }
    }))
}

async fn readiness(axum::extract::State(state): axum::extract::State<AppState>) -> Response {
    readiness_response(&state).await
}

async fn readiness_response(state: &AppState) -> Response {
    let database_ready = match sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.db)
        .await
    {
        Ok(_) => true,
        Err(err) => {
            tracing::warn!(error = ?err, "configuration database readiness probe failed");
            false
        }
    };
    let storage_ready = match backup_storage_ready(&state.config.backup_dir).await {
        Ok(()) => true,
        Err(err) => {
            tracing::warn!(
                path = %state.config.backup_dir.display(),
                error = ?err,
                "backup storage readiness probe failed"
            );
            false
        }
    };
    let ready = database_ready && storage_ready;
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        status,
        Json(json!({
            "data": {
                "status": if ready { "ready" } else { "unavailable" },
                "checks": {
                    "database": if database_ready { "ok" } else { "unavailable" },
                    "backup_storage": if storage_ready { "ok" } else { "unavailable" }
                }
            }
        })),
    )
        .into_response()
}

async fn backup_storage_ready(backup_dir: &Path) -> std::io::Result<()> {
    tokio::fs::create_dir_all(backup_dir).await?;
    let probe_path = backup_dir.join(format!(".dumply-readiness-{}", Uuid::new_v4()));
    let mut probe = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&probe_path)
        .await?;
    if let Err(err) = probe.write_all(b"ready").await {
        drop(probe);
        let _ = tokio::fs::remove_file(&probe_path).await;
        return Err(err);
    }
    drop(probe);
    tokio::fs::remove_file(probe_path).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn storage_readiness_probe_leaves_no_artifact() {
        let directory = tempfile::tempdir().unwrap();
        backup_storage_ready(directory.path()).await.unwrap();
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}
