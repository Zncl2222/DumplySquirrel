use std::{collections::HashMap, sync::Arc};

use tokio::sync::Mutex;
use tokio_cron_scheduler::{Job, JobScheduler};
use uuid::Uuid;

use crate::{
    db::models::BackupConfig,
    error::{AppError, AppResult},
    services::backup_executor::{self, BackupRuntime, BackupTrigger},
};

pub struct BackupScheduler {
    scheduler: JobScheduler,
    runtime: BackupRuntime,
    jobs: Arc<Mutex<HashMap<Uuid, Uuid>>>,
}

impl BackupScheduler {
    pub async fn new(runtime: BackupRuntime) -> anyhow::Result<Self> {
        Ok(Self {
            scheduler: JobScheduler::new().await?,
            runtime,
            jobs: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub async fn start(&self) -> anyhow::Result<()> {
        self.scheduler.start().await?;
        Ok(())
    }

    pub async fn reload_all(&self) -> AppResult<()> {
        let existing_job_ids = {
            let mut jobs = self.jobs.lock().await;
            jobs.drain().map(|(_, job_id)| job_id).collect::<Vec<_>>()
        };

        for job_id in existing_job_ids {
            self.scheduler
                .remove(&job_id)
                .await
                .map_err(|err| AppError::Internal(err.into()))?;
        }

        let configs = sqlx::query_as::<_, BackupConfig>(
            r#"
            SELECT id, name, db_type, db_version, db_url_encrypted, db_url_nonce, cron_schedule,
                   is_enabled, retention_days, timeout_seconds, max_backups,
                   email_to, email_cc, email_notify_on,
                   created_by, created_at, updated_at
            FROM backup_configs
            WHERE is_enabled = true AND cron_schedule IS NOT NULL
            "#,
        )
        .fetch_all(&self.runtime.db)
        .await?;

        for config in configs {
            self.schedule_config(config).await?;
        }

        Ok(())
    }

    pub async fn refresh_config(&self, config_id: Uuid) -> AppResult<()> {
        self.remove_config(config_id).await?;
        let config = sqlx::query_as::<_, BackupConfig>(
            r#"
            SELECT id, name, db_type, db_version, db_url_encrypted, db_url_nonce, cron_schedule,
                   is_enabled, retention_days, timeout_seconds, max_backups,
                   email_to, email_cc, email_notify_on,
                   created_by, created_at, updated_at
            FROM backup_configs
            WHERE id = $1
            "#,
        )
        .bind(config_id)
        .fetch_optional(&self.runtime.db)
        .await?;

        if let Some(config) = config {
            self.schedule_config(config).await?;
        }

        Ok(())
    }

    pub async fn remove_config(&self, config_id: Uuid) -> AppResult<()> {
        let job_id = {
            let mut jobs = self.jobs.lock().await;
            jobs.remove(&config_id)
        };

        if let Some(job_id) = job_id {
            self.scheduler
                .remove(&job_id)
                .await
                .map_err(|err| AppError::Internal(err.into()))?;
        }

        Ok(())
    }

    async fn schedule_config(&self, config: BackupConfig) -> AppResult<()> {
        let Some(schedule) = config.cron_schedule.clone() else {
            return Ok(());
        };
        if !config.is_enabled {
            return Ok(());
        }

        let config_id = config.id;
        let runtime = self.runtime.clone();
        let job = Job::new_async(schedule.as_str(), move |_job_id, _lock| {
            let runtime = runtime.clone();
            Box::pin(async move {
                match backup_executor::spawn_backup(runtime, config_id, BackupTrigger::Scheduled)
                    .await
                {
                    Ok(history) => tracing::info!(
                        config_id = %config_id,
                        history_id = %history.id,
                        "scheduled backup started"
                    ),
                    Err(err) => tracing::warn!(
                        config_id = %config_id,
                        error = ?err,
                        "scheduled backup skipped or failed to start"
                    ),
                }
            })
        })
        .map_err(|err| AppError::Validation(format!("invalid cron_schedule: {err}")))?;
        let job_id = job.guid();
        self.scheduler
            .add(job)
            .await
            .map_err(|err| AppError::Internal(err.into()))?;

        let mut jobs = self.jobs.lock().await;
        jobs.insert(config_id, job_id);
        tracing::info!(config_id = %config_id, schedule = %schedule, "scheduled backup registered");
        Ok(())
    }
}

pub fn validate_cron_expression(schedule: &str) -> AppResult<()> {
    Job::new_async(schedule, |_job_id, _lock| Box::pin(async {}))
        .map_err(|err| AppError::Validation(format!("invalid cron_schedule: {err}")))?;
    Ok(())
}
