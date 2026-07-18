use std::{path::Path, path::PathBuf, time::Duration};

use sqlx::PgPool;
use tokio::time::MissedTickBehavior;
use uuid::Uuid;

use crate::{
    error::AppResult,
    services::backup_executor::{self, ManagedFileRemoval},
};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FileDeletionPurgeReport {
    pub deleted: u64,
    pub retained: u64,
}

const PURGE_BATCH_SIZE: i64 = 100;
const PURGE_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, sqlx::FromRow)]
struct PendingFileDeletion {
    id: Uuid,
    file_path: String,
}

/// Retries durable file-deletion intents left by config deletion.
///
/// A queue row is removed only after the managed file was unlinked or was already absent.
/// Unsafe paths and transient I/O failures stay queued with attempt diagnostics.
pub async fn purge_pending_file_deletions(
    pool: &PgPool,
    backup_dir: &Path,
) -> AppResult<FileDeletionPurgeReport> {
    let pending = sqlx::query_as::<_, PendingFileDeletion>(
        r#"
        SELECT id, file_path
        FROM pending_file_deletions
        WHERE next_attempt_at <= now()
        ORDER BY created_at, id
        LIMIT $1
        "#,
    )
    .bind(PURGE_BATCH_SIZE)
    .fetch_all(pool)
    .await?;

    let mut report = FileDeletionPurgeReport::default();
    for deletion in pending {
        match backup_executor::remove_managed_file(backup_dir, Path::new(&deletion.file_path)).await
        {
            Ok(ManagedFileRemoval::Removed | ManagedFileRemoval::Missing) => {
                sqlx::query("DELETE FROM pending_file_deletions WHERE id = $1")
                    .bind(deletion.id)
                    .execute(pool)
                    .await?;
                report.deleted += 1;
            }
            Ok(ManagedFileRemoval::OutsideBackupDir) => {
                retain_failed_deletion(
                    pool,
                    &deletion,
                    "path is outside the configured backup directory",
                )
                .await?;
                report.retained += 1;
            }
            Ok(ManagedFileRemoval::NotAFile) => {
                retain_failed_deletion(pool, &deletion, "managed path is not a file").await?;
                report.retained += 1;
            }
            Err(err) => {
                retain_failed_deletion(pool, &deletion, &format!("file removal failed: {err}"))
                    .await?;
                report.retained += 1;
            }
        }
    }

    Ok(report)
}

async fn retain_failed_deletion(
    pool: &PgPool,
    deletion: &PendingFileDeletion,
    error: &str,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE pending_file_deletions
        SET attempts = attempts + 1,
            last_error = $2,
            last_attempt_at = now(),
            next_attempt_at = now()
                + (LEAST(3600, 30 * (attempts + 1)) * interval '1 second')
        WHERE id = $1
        "#,
    )
    .bind(deletion.id)
    .bind(error)
    .execute(pool)
    .await?;
    tracing::warn!(
        deletion_id = %deletion.id,
        path = %deletion.file_path,
        error,
        "pending backup file deletion retained for retry"
    );
    Ok(())
}

/// Runs bounded deletion batches outside HTTP requests. Due work is retried every 30 seconds;
/// failures use a database-backed increasing delay and survive process restarts.
pub fn spawn_file_deletion_worker(
    pool: PgPool,
    backup_dir: PathBuf,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(PURGE_INTERVAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            loop {
                match purge_pending_file_deletions(&pool, &backup_dir).await {
                    Ok(report) => {
                        let examined = report.deleted + report.retained;
                        if examined > 0 {
                            tracing::info!(
                                deleted = report.deleted,
                                retained = report.retained,
                                "pending backup-file deletion batch completed"
                            );
                        }
                        if examined < PURGE_BATCH_SIZE as u64 {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                    Err(err) => {
                        tracing::warn!(error = ?err, "pending backup-file deletion worker failed");
                        break;
                    }
                }
            }
        }
    })
}
