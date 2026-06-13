use axum::{extract::State, http::HeaderMap, routing::get, Json, Router};
use serde_json::json;

use crate::{error::AppResult, AppState};

pub fn router() -> Router<AppState> {
    Router::new().route("/stats", get(stats))
}

async fn stats(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<serde_json::Value>> {
    state.auth.authorize(&headers, &state.config)?;

    let total_configs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backup_configs")
        .fetch_one(&state.db)
        .await?;
    let total_backups: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backup_history")
        .fetch_one(&state.db)
        .await?;
    let success_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM backup_history WHERE status = 'success'")
            .fetch_one(&state.db)
            .await?;
    let failed_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM backup_history WHERE status IN ('failed', 'timeout')",
    )
    .fetch_one(&state.db)
    .await?;
    let storage_bytes: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(file_size), 0)::BIGINT FROM backup_history WHERE status = 'success'",
    )
    .fetch_one(&state.db)
    .await?;

    Ok(Json(json!({
        "data": {
            "total_configs": total_configs,
            "total_backups": total_backups,
            "success_count": success_count,
            "failed_count": failed_count,
            "storage_bytes": storage_bytes
        }
    })))
}
