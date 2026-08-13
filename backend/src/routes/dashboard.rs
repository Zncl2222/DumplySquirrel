use axum::{extract::State, http::HeaderMap, routing::get, Json, Router};
use serde::Serialize;
use serde_json::json;

use crate::{error::AppResult, AppState};

pub fn router() -> Router<AppState> {
    Router::new().route("/stats", get(stats))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct DashboardStats {
    total_configs: i64,
    protected_configs: i64,
    total_backups: i64,
    success_count: i64,
    failed_count: i64,
    cancelled_count: i64,
    running_count: i64,
    storage_bytes: i64,
    pending_file_deletions: i64,
    last_success_at: Option<chrono::DateTime<chrono::Utc>>,
}

async fn stats(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<serde_json::Value>> {
    state
        .auth
        .authorize_active(&headers, &state.config, &state.db)
        .await?;

    let dashboard = load_dashboard_stats(&state.db).await?;

    Ok(Json(json!({ "data": dashboard })))
}

async fn load_dashboard_stats(pool: &sqlx::PgPool) -> AppResult<DashboardStats> {
    let dashboard = sqlx::query_as::<_, DashboardStats>(
        r#"
        WITH history_stats AS (
            SELECT
                COUNT(*) AS total_backups,
                COUNT(DISTINCT config_id) FILTER (
                    WHERE status = 'success' AND file_path IS NOT NULL
                ) AS protected_configs,
                COUNT(*) FILTER (WHERE status = 'success') AS success_count,
                COUNT(*) FILTER (WHERE status IN ('failed', 'timeout')) AS failed_count,
                COUNT(*) FILTER (WHERE status = 'cancelled') AS cancelled_count,
                COUNT(*) FILTER (WHERE status = 'running') AS running_count,
                COALESCE(SUM(file_size) FILTER (
                    WHERE status = 'success' AND file_path IS NOT NULL
                ), 0)::BIGINT AS storage_bytes,
                MAX(completed_at) FILTER (WHERE status = 'success') AS last_success_at
            FROM backup_history
        )
        SELECT
            (SELECT COUNT(*) FROM backup_configs) AS total_configs,
            history_stats.protected_configs,
            history_stats.total_backups,
            history_stats.success_count,
            history_stats.failed_count,
            history_stats.cancelled_count,
            history_stats.running_count,
            history_stats.storage_bytes,
            (SELECT COUNT(*) FROM pending_file_deletions) AS pending_file_deletions,
            history_stats.last_success_at
        FROM history_stats
        "#,
    )
    .fetch_one(pool)
    .await?;

    Ok(dashboard)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL pointing to a disposable PostgreSQL database"]
    async fn dashboard_stats_count_available_protection_and_terminal_outcomes() {
        let database_url = std::env::var("TEST_DATABASE_URL")
            .expect("TEST_DATABASE_URL must point to a disposable PostgreSQL database");
        let pool = crate::db::connect(&database_url).await.unwrap();
        crate::db::migrate(&pool).await.unwrap();
        let baseline = load_dashboard_stats(&pool).await.unwrap();
        let protected_config_id = Uuid::new_v4();
        let retained_config_id = Uuid::new_v4();

        for config_id in [protected_config_id, retained_config_id] {
            sqlx::query(
                r#"
                INSERT INTO backup_configs
                    (id, name, db_type, db_url_encrypted, db_url_nonce, is_enabled)
                VALUES ($1, $2, 'postgres', 'unused', 'unused', false)
                "#,
            )
            .bind(config_id)
            .bind(format!("dashboard-stats-{config_id}"))
            .execute(&pool)
            .await
            .unwrap();
        }

        sqlx::query(
            r#"
            INSERT INTO backup_history
                (id, config_id, status, file_name, file_size, file_path,
                 started_at, completed_at, triggered_by)
            VALUES
                ($1, $2, 'success', 'available.sql', 12, '/backups/available.sql',
                 now() - interval '3 minutes', now() - interval '2 minutes', 'manual'),
                ($3, $4, 'success', 'retained-away.sql', NULL, NULL,
                 now() - interval '2 minutes', now() - interval '1 minute', 'scheduled'),
                ($5, $4, 'cancelled', NULL, NULL, NULL,
                 now() - interval '1 minute', now(), 'manual')
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(protected_config_id)
        .bind(Uuid::new_v4())
        .bind(retained_config_id)
        .bind(Uuid::new_v4())
        .execute(&pool)
        .await
        .unwrap();

        let stats = load_dashboard_stats(&pool).await.unwrap();
        assert_eq!(stats.total_configs, baseline.total_configs + 2);
        assert_eq!(stats.protected_configs, baseline.protected_configs + 1);
        assert_eq!(stats.total_backups, baseline.total_backups + 3);
        assert_eq!(stats.success_count, baseline.success_count + 2);
        assert_eq!(stats.cancelled_count, baseline.cancelled_count + 1);
        assert_eq!(stats.storage_bytes, baseline.storage_bytes + 12);
        assert!(stats.last_success_at >= baseline.last_success_at);

        sqlx::query("DELETE FROM backup_configs WHERE id = ANY($1)")
            .bind(vec![protected_config_id, retained_config_id])
            .execute(&pool)
            .await
            .unwrap();
    }
}
