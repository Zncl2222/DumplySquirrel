use std::{
    fs::{File, OpenOptions},
    io,
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use chrono::Utc;
use percent_encoding::percent_decode_str;
use sqlx::PgPool;
use tempfile::NamedTempFile;
use tokio::{io::AsyncReadExt, process::Command, sync::Semaphore};
use uuid::Uuid;

use crate::{
    config::AppConfig,
    db::models::{BackupConfig, BackupHistory},
    error::{AppError, AppResult},
    services::{crypto, email_notifier},
};

#[derive(Debug, Clone, Copy)]
enum BackupFinalStatus {
    Success,
    Timeout,
}

impl BackupFinalStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Timeout => "timeout",
        }
    }
}

#[derive(Clone)]
pub struct BackupRuntime {
    pub db: PgPool,
    pub config: Arc<AppConfig>,
    pub permits: Arc<Semaphore>,
}

#[derive(Debug, Clone, Copy)]
pub enum BackupTrigger {
    Manual,
    Scheduled,
}

impl BackupTrigger {
    fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Scheduled => "scheduled",
        }
    }
}

pub async fn spawn_backup(
    runtime: BackupRuntime,
    config_id: Uuid,
    trigger: BackupTrigger,
) -> AppResult<BackupHistory> {
    let (config, history) = prepare_backup(&runtime.db, config_id, trigger).await?;
    let history_id = history.id;

    tokio::spawn(async move {
        if let Err(err) = run_backup(runtime, config, history_id).await {
            tracing::error!(history_id = %history_id, error = ?err, "backup worker failed");
        }
    });

    Ok(history)
}

async fn prepare_backup(
    pool: &PgPool,
    config_id: Uuid,
    trigger: BackupTrigger,
) -> AppResult<(BackupConfig, BackupHistory)> {
    let mut transaction = pool.begin().await?;
    // Deletion takes the same parent-row lock before checking running histories.
    // Keeping it through this insert makes either operation observe the other's result.
    let config = sqlx::query_as::<_, BackupConfig>(
        r#"
        SELECT id, name, db_type, db_version, db_url_encrypted, db_url_nonce, cron_schedule,
               is_enabled, retention_days, timeout_seconds, max_backups,
               email_to, email_cc, email_notify_on,
               created_by, created_at, updated_at
        FROM backup_configs
        WHERE id = $1
        FOR UPDATE
        "#,
    )
    .bind(config_id)
    .fetch_one(&mut *transaction)
    .await?;

    let result = sqlx::query_as::<_, BackupHistory>(
        r#"
        INSERT INTO backup_history (id, config_id, status, started_at, triggered_by)
        VALUES ($1, $2, 'running', $3, $4)
        RETURNING id, config_id, status, file_name, file_size, file_path,
                  false AS is_downloadable,
                  error_message, started_at, completed_at, triggered_by
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(config_id)
    .bind(Utc::now())
    .bind(trigger.as_str())
    .fetch_one(&mut *transaction)
    .await;

    let history = match result {
        Ok(history) => history,
        Err(sqlx::Error::Database(err)) if err.code().as_deref() == Some("23505") => Err(
            AppError::Conflict("backup is already running for this config".into()),
        )?,
        Err(err) => return Err(err.into()),
    };

    transaction.commit().await?;
    Ok((config, history))
}

async fn run_backup(
    runtime: BackupRuntime,
    config: BackupConfig,
    history_id: Uuid,
) -> AppResult<()> {
    backup_event(
        &runtime.db,
        history_id,
        "queued",
        "info",
        "Backup worker queued for an execution slot.",
    )
    .await;
    let permit_result = tokio::time::timeout(
        Duration::from_secs(30),
        runtime.permits.clone().acquire_owned(),
    )
    .await;
    let permit = match permit_result {
        Ok(Ok(permit)) => permit,
        Ok(Err(err)) => {
            let err = AppError::Internal(err.into());
            backup_event(&runtime.db, history_id, "queued", "error", &err.to_string()).await;
            mark_failed_if_running(&runtime.db, history_id, "failed", &err.to_string()).await?;
            email_notifier::notify_backup_completed(
                &runtime.db,
                &runtime.config,
                &config,
                history_id,
                "failed",
            )
            .await;
            return Err(err);
        }
        Err(_) => {
            let err = AppError::Conflict("backup queue is full; try again later".into());
            backup_event(&runtime.db, history_id, "queued", "error", &err.to_string()).await;
            mark_failed_if_running(&runtime.db, history_id, "failed", &err.to_string()).await?;
            email_notifier::notify_backup_completed(
                &runtime.db,
                &runtime.config,
                &config,
                history_id,
                "failed",
            )
            .await;
            return Err(err);
        }
    };
    backup_event(
        &runtime.db,
        history_id,
        "queued",
        "success",
        "Execution slot acquired.",
    )
    .await;

    let result = execute_backup(&runtime, &config, history_id).await;
    drop(permit);

    match result {
        Ok(status) => {
            email_notifier::notify_backup_completed(
                &runtime.db,
                &runtime.config,
                &config,
                history_id,
                status.as_str(),
            )
            .await;
        }
        Err(err) => {
            if matches!(err, AppError::BackupCommitPending(_)) {
                // The final file is the durable commit marker. Keep the row running so startup
                // recovery can commit it; marking failure here would orphan a valid backup.
                return Err(err);
            }
            let status = if matches!(err, AppError::BackupTimeout) {
                "timeout"
            } else {
                "failed"
            };
            mark_failed_if_running(&runtime.db, history_id, status, &err.to_string()).await?;
            email_notifier::notify_backup_completed(
                &runtime.db,
                &runtime.config,
                &config,
                history_id,
                status,
            )
            .await;
            return Err(err);
        }
    }

    Ok(())
}

const RESTART_INTERRUPTION_MESSAGE: &str =
    "Backup interrupted because the service restarted before completion.";
const RESTART_COMMIT_MESSAGE: &str =
    "Backup file was already finalized; startup recovery committed the run successfully.";

/// Marks runs abandoned by a previous process as terminal before the scheduler starts.
///
/// New workers persist both their random partial path and intended final file name while they
/// are running. Keeping those two values lets recovery cover a crash on either side of the
/// atomic rename. The directory sweep also removes recognizable partial files from older runs
/// whose history row was never updated.
pub async fn recover_interrupted_backups(runtime: &BackupRuntime) -> AppResult<u64> {
    tokio::fs::create_dir_all(&runtime.config.backup_dir)
        .await
        .map_err(|err| AppError::Internal(err.into()))?;

    let interrupted = sqlx::query_as::<_, InterruptedBackup>(
        r#"
        SELECT id, file_name, file_path
        FROM backup_history
        WHERE status = 'running'
        ORDER BY started_at
        "#,
    )
    .fetch_all(&runtime.db)
    .await?;

    let mut recovered = 0_u64;
    let mut committed = 0_u64;
    for backup in &interrupted {
        // A successful atomic rename is the durable commit marker. Inspect it before removing
        // anything so a crash between rename and the normal database update is recovered as a
        // successful backup rather than deleting the completed artifact.
        let finalized = find_durable_final_output(&runtime.config.backup_dir, backup)
            .await
            .map_err(|err| AppError::Internal(err.into()))?;
        // Finish partial-file cleanup before the compare-and-set. If recovery itself stops here,
        // the still-running row and durable final file make the decision repeatable next start.
        cleanup_interrupted_partial(&runtime.config.backup_dir, backup)
            .await
            .map_err(|err| AppError::Internal(err.into()))?;

        let (result, event_level, event_message) = match finalized {
            Some(finalized) => {
                let result = sqlx::query(
                    r#"
                    UPDATE backup_history
                    SET status = 'success', error_message = NULL, completed_at = now(),
                        file_path = $2, file_size = $3
                    WHERE id = $1 AND status = 'running'
                    "#,
                )
                .bind(backup.id)
                .bind(finalized.path.to_string_lossy().to_string())
                .bind(finalized.size)
                .execute(&runtime.db)
                .await?;
                (result, "success", RESTART_COMMIT_MESSAGE)
            }
            None => {
                let result = sqlx::query(
                    r#"
                    UPDATE backup_history
                    SET status = 'failed', error_message = $2, completed_at = now(),
                        file_path = NULL, file_size = NULL
                    WHERE id = $1 AND status = 'running'
                    "#,
                )
                .bind(backup.id)
                .bind(RESTART_INTERRUPTION_MESSAGE)
                .execute(&runtime.db)
                .await?;
                (result, "error", RESTART_INTERRUPTION_MESSAGE)
            }
        };
        if result.rows_affected() == 1 {
            recovered += 1;
            if event_level == "success" {
                committed += 1;
            }
            backup_event(
                &runtime.db,
                backup.id,
                "interrupted",
                event_level,
                event_message,
            )
            .await;
        }
    }

    sweep_partial_outputs(&runtime.config.backup_dir).await;

    if recovered > 0 {
        tracing::warn!(
            count = recovered,
            committed,
            failed = recovered - committed,
            "recovered backup runs interrupted by a previous process"
        );
    }
    Ok(recovered)
}

async fn execute_backup(
    runtime: &BackupRuntime,
    config: &BackupConfig,
    history_id: Uuid,
) -> AppResult<BackupFinalStatus> {
    tokio::fs::create_dir_all(&runtime.config.backup_dir)
        .await
        .map_err(|err| AppError::Internal(err.into()))?;
    backup_event(
        &runtime.db,
        history_id,
        "prepare",
        "success",
        "Backup directory is ready.",
    )
    .await;

    let db_url = crypto::decrypt_string(
        &config.db_url_encrypted,
        &config.db_url_nonce,
        &runtime.config.database_encryption_key,
    )?;
    let target = DumpTarget::parse(&db_url)?;
    target.validate_db_type(&config.db_type)?;
    backup_event(
        &runtime.db,
        history_id,
        "connect",
        "success",
        &format!(
            "Target accepted for {} on {}:{}.",
            config.db_type, target.host, target.port
        ),
    )
    .await;
    let file_name = format!(
        "{}_{}_{}.sql",
        config.id,
        Utc::now().format("%Y%m%d_%H%M%S"),
        history_id
    );
    let output_path = runtime.config.backup_dir.join(&file_name);
    let (partial_path, output_file) =
        reserve_partial_output(&runtime.config.backup_dir, history_id)?;
    if let Err(err) =
        record_pending_output(&runtime.db, history_id, &file_name, &partial_path).await
    {
        cleanup_partial_file(&partial_path).await;
        return Err(err);
    }
    backup_event(
        &runtime.db,
        history_id,
        "prepare",
        "info",
        &format!("Output file reserved as `{file_name}`."),
    )
    .await;

    let dump_result = match config.db_type.as_str() {
        "postgres" => {
            run_pg_dump(
                &runtime.db,
                history_id,
                &target,
                output_file,
                config.db_version.as_deref(),
                config.timeout_seconds,
            )
            .await
        }
        "mysql" => {
            run_mysqldump(
                &runtime.db,
                history_id,
                &target,
                output_file,
                config.db_version.as_deref(),
                config.timeout_seconds,
            )
            .await
        }
        other => Err(AppError::Validation(format!("unsupported db_type {other}"))),
    };

    match dump_result {
        Ok(()) => {
            if let Err(err) = finalize_partial_output(&partial_path, &output_path).await {
                if matches!(err, AppError::BackupCommitPending(_)) {
                    // Rename succeeded, but its directory entry was not proven durable. Leave
                    // the row running so startup recovery decides from the actual final file.
                    backup_event(&runtime.db, history_id, "seal", "warning", &err.to_string())
                        .await;
                    return Err(err);
                }
                cleanup_partial_file(&partial_path).await;
                backup_event(&runtime.db, history_id, "seal", "error", &err.to_string()).await;
                mark_failed(&runtime.db, history_id, "failed", &err.to_string(), None).await?;
                return Err(err);
            }
            let file_size = match tokio::fs::metadata(&output_path).await {
                Ok(metadata) => metadata.len() as i64,
                Err(source) => {
                    return Err(AppError::BackupCommitPending(format!(
                        "failed to inspect finalized output {}: {source}",
                        output_path.display()
                    )));
                }
            };
            backup_event(
                &runtime.db,
                history_id,
                "seal",
                "success",
                &format!("Backup file sealed at {file_size} bytes."),
            )
            .await;
            if let Err(err) = mark_success_with_retry(
                &runtime.db,
                history_id,
                &file_name,
                &output_path,
                file_size,
            )
            .await
            {
                return Err(AppError::BackupCommitPending(err.to_string()));
            }
            backup_event(
                &runtime.db,
                history_id,
                "retention",
                "info",
                "Applying retention rules.",
            )
            .await;
            match apply_retention(runtime, config.id).await {
                Ok(()) => {
                    backup_event(
                        &runtime.db,
                        history_id,
                        "retention",
                        "success",
                        "Retention rules applied.",
                    )
                    .await;
                }
                Err(err) => {
                    tracing::warn!(history_id = %history_id, error = ?err, "retention maintenance failed");
                    backup_event(
                        &runtime.db,
                        history_id,
                        "retention",
                        "warning",
                        &format!(
                            "Backup is valid, but retention maintenance failed and should be retried: {err}"
                        ),
                    )
                    .await;
                }
            }
            backup_event(
                &runtime.db,
                history_id,
                "done",
                "success",
                "Backup completed successfully.",
            )
            .await;
            Ok(BackupFinalStatus::Success)
        }
        Err(AppError::BackupTimeout) => {
            cleanup_partial_file(&partial_path).await;
            backup_event(
                &runtime.db,
                history_id,
                "timeout",
                "error",
                "Backup process timed out. Partial file was removed.",
            )
            .await;
            mark_failed(
                &runtime.db,
                history_id,
                "timeout",
                "backup process timed out",
                None,
            )
            .await?;
            Ok(BackupFinalStatus::Timeout)
        }
        Err(err) => {
            cleanup_partial_file(&partial_path).await;
            backup_event(&runtime.db, history_id, "failed", "error", &err.to_string()).await;
            mark_failed(&runtime.db, history_id, "failed", &err.to_string(), None).await?;
            Err(err)
        }
    }
}

fn reserve_partial_output(backup_dir: &Path, history_id: Uuid) -> AppResult<(PathBuf, File)> {
    const MAX_ATTEMPTS: usize = 4;

    for _ in 0..MAX_ATTEMPTS {
        let path = partial_output_path(backup_dir, history_id);
        match open_new_output_file(&path) {
            Ok(file) => return Ok((path, file)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(AppError::Internal(err.into())),
        }
    }

    Err(AppError::Internal(anyhow::anyhow!(
        "failed to reserve a unique partial backup file"
    )))
}

fn partial_output_path(backup_dir: &Path, history_id: Uuid) -> PathBuf {
    backup_dir.join(format!(".dumply-{history_id}-{}.part", Uuid::new_v4()))
}

fn open_new_output_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    options.open(path)
}

async fn record_pending_output(
    pool: &PgPool,
    history_id: Uuid,
    file_name: &str,
    partial_path: &Path,
) -> AppResult<()> {
    let result = sqlx::query(
        r#"
        UPDATE backup_history
        SET file_name = $2, file_path = $3
        WHERE id = $1 AND status = 'running'
        "#,
    )
    .bind(history_id)
    .bind(file_name)
    .bind(partial_path.to_string_lossy().to_string())
    .execute(pool)
    .await?;

    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(AppError::Conflict(
            "backup is no longer in a running state".into(),
        ))
    }
}

async fn finalize_partial_output(partial_path: &Path, output_path: &Path) -> AppResult<()> {
    match tokio::fs::symlink_metadata(output_path).await {
        Ok(_) => {
            return Err(AppError::Conflict(format!(
                "backup output already exists at {}",
                output_path.display()
            )))
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(AppError::Internal(err.into())),
    }

    let partial_file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(partial_path)
        .await
        .map_err(|err| AppError::Internal(err.into()))?;
    partial_file
        .sync_all()
        .await
        .map_err(|err| AppError::Internal(err.into()))?;
    drop(partial_file);

    tokio::fs::rename(partial_path, output_path)
        .await
        .map_err(|err| AppError::Internal(err.into()))?;

    #[cfg(unix)]
    {
        let parent = output_path.parent().ok_or_else(|| {
            AppError::BackupCommitPending(format!(
                "finalized output has no parent directory: {}",
                output_path.display()
            ))
        })?;
        let directory = tokio::fs::File::open(parent).await.map_err(|source| {
            AppError::BackupCommitPending(format!(
                "failed to open backup directory {} after finalizing output: {source}",
                parent.display()
            ))
        })?;
        directory.sync_all().await.map_err(|source| {
            AppError::BackupCommitPending(format!(
                "failed to sync backup directory {} after finalizing output: {source}",
                parent.display()
            ))
        })?;
    }
    Ok(())
}

async fn run_pg_dump(
    pool: &PgPool,
    history_id: Uuid,
    target: &DumpTarget,
    output: File,
    configured_version: Option<&str>,
    timeout_seconds: i32,
) -> AppResult<()> {
    let configured_version = configured_version
        .map(str::trim)
        .filter(|version| !version.is_empty() && !version.eq_ignore_ascii_case("auto"));
    let major_version = match configured_version {
        Some(version) => {
            backup_event(
                pool,
                history_id,
                "probe",
                "info",
                &format!("Using configured PostgreSQL {version}."),
            )
            .await;
            version.trim().to_string()
        }
        None => {
            backup_event(
                pool,
                history_id,
                "probe",
                "info",
                "Detecting PostgreSQL server version.",
            )
            .await;
            let version = detect_postgres_major_version(target, timeout_seconds).await?;
            backup_event(
                pool,
                history_id,
                "probe",
                "success",
                &format!("Detected PostgreSQL server version {version}."),
            )
            .await;
            version
        }
    };
    let command_path = resolve_pg_dump_command(&major_version).await?;
    tracing::info!(postgres_version = %major_version, pg_dump = %command_path, "using PostgreSQL pg_dump client");
    backup_event(
        pool,
        history_id,
        "client",
        "success",
        &format!("Using `{command_path}` for PostgreSQL {major_version}."),
    )
    .await;
    let mut command = Command::new(&command_path);
    command
        .arg("-h")
        .arg(&target.host)
        .arg("-p")
        .arg(target.port.to_string())
        .arg("-U")
        .arg(&target.username)
        .arg("-d")
        .arg(&target.database)
        .stdout(Stdio::from(output))
        .stderr(Stdio::piped());
    if let Some(password) = &target.password {
        command.env("PGPASSWORD", password);
    }

    backup_event(pool, history_id, "dump", "info", "pg_dump process started.").await;
    run_command(&command_path, command, timeout_seconds).await?;
    backup_event(
        pool,
        history_id,
        "dump",
        "success",
        "pg_dump process exited successfully.",
    )
    .await;
    Ok(())
}

async fn run_mysqldump(
    pool: &PgPool,
    history_id: Uuid,
    target: &DumpTarget,
    output: File,
    configured_version: Option<&str>,
    timeout_seconds: i32,
) -> AppResult<()> {
    let configured_version = configured_version
        .map(str::trim)
        .filter(|version| !version.is_empty() && !version.eq_ignore_ascii_case("auto"));
    let version = match configured_version {
        Some(version) => {
            backup_event(
                pool,
                history_id,
                "probe",
                "info",
                &format!("Using configured MySQL {version}."),
            )
            .await;
            version.trim().to_string()
        }
        None => {
            backup_event(
                pool,
                history_id,
                "probe",
                "info",
                "Detecting MySQL server version.",
            )
            .await;
            let version = detect_mysql_version(target, timeout_seconds).await?;
            backup_event(
                pool,
                history_id,
                "probe",
                "success",
                &format!("Detected MySQL server version {version}."),
            )
            .await;
            version
        }
    };
    tracing::info!(mysql_version = %version, "using system mysqldump for MySQL backup");
    backup_event(
        pool,
        history_id,
        "client",
        "success",
        &format!("Using system `mysqldump` for MySQL {version}."),
    )
    .await;
    let option_file = mysql_option_file(target)?;
    let option_path = option_file.path().to_path_buf();
    let mut command = Command::new("mysqldump");
    command
        .arg(format!("--defaults-extra-file={}", option_path.display()))
        .arg("--")
        .arg(&target.database)
        .stdout(Stdio::from(output))
        .stderr(Stdio::piped());

    backup_event(
        pool,
        history_id,
        "dump",
        "info",
        "mysqldump process started.",
    )
    .await;
    let result = run_command("mysqldump", command, timeout_seconds).await;
    drop(option_file);
    if result.is_ok() {
        backup_event(
            pool,
            history_id,
            "dump",
            "success",
            "mysqldump process exited successfully.",
        )
        .await;
    }
    result
}

async fn detect_postgres_major_version(
    target: &DumpTarget,
    timeout_seconds: i32,
) -> AppResult<String> {
    let mut command = Command::new("psql");
    command
        .arg("-h")
        .arg(&target.host)
        .arg("-p")
        .arg(target.port.to_string())
        .arg("-U")
        .arg(&target.username)
        .arg("-d")
        .arg(&target.database)
        .arg("-tA")
        .arg("-c")
        .arg("SHOW server_version_num")
        .stderr(Stdio::piped())
        .stdout(Stdio::piped());
    if let Some(password) = &target.password {
        command.env("PGPASSWORD", password);
    }

    let output = run_command_output("psql", command, timeout_seconds).await?;
    postgres_major_from_server_version_num(output.trim())
}

async fn resolve_pg_dump_command(major_version: &str) -> AppResult<String> {
    let managed_path = format!("/usr/lib/postgresql/{major_version}/bin/pg_dump");
    let mut candidates = Vec::new();
    if Path::new(&managed_path).exists() {
        candidates.push(managed_path);
    }
    candidates.push("pg_dump".to_string());

    for candidate in candidates {
        match command_postgres_major(&candidate).await {
            Ok(candidate_major) if candidate_major == major_version => return Ok(candidate),
            _ => {}
        }
    }

    Err(AppError::Internal(anyhow::anyhow!(
        "pg_dump client for PostgreSQL {major_version} is not installed; install postgresql-client-{major_version} or put a matching pg_dump in PATH"
    )))
}

async fn command_postgres_major(command_path: &str) -> AppResult<String> {
    let mut command = Command::new(command_path);
    command.arg("--version");
    let output = run_command_output(command_path, command, 10).await?;
    postgres_major_from_pg_dump_version(&output)
}

async fn detect_mysql_version(target: &DumpTarget, timeout_seconds: i32) -> AppResult<String> {
    let option_file = mysql_option_file(target)?;
    let option_path = option_file.path().to_path_buf();
    let mut command = Command::new("mysql");
    command
        .arg(format!("--defaults-extra-file={}", option_path.display()))
        .arg("-N")
        .arg("-B")
        .arg(&target.database)
        .arg("-e")
        .arg("SELECT VERSION()")
        .stderr(Stdio::piped())
        .stdout(Stdio::piped());

    let result = run_command_output("mysql", command, timeout_seconds).await;
    drop(option_file);
    result.map(|output| output.trim().to_string())
}

async fn run_command_output(
    command_name: &str,
    mut command: Command,
    timeout_seconds: i32,
) -> AppResult<String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    configure_child_process(&mut command);
    let mut child = command
        .spawn()
        .map_err(|err| command_spawn_error(command_name, err))?;
    let mut stdout_task = tokio::spawn(read_stream(child.stdout.take()));
    let mut stderr_task = tokio::spawn(read_stream(child.stderr.take()));
    let status = match tokio::time::timeout(
        Duration::from_secs(timeout_seconds.max(1) as u64),
        child.wait(),
    )
    .await
    {
        Ok(result) => result.map_err(|err| AppError::Internal(err.into()))?,
        Err(_) => {
            terminate_child_process(&mut child).await;
            stdout_task.abort();
            stderr_task.abort();
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            return Err(AppError::BackupTimeout);
        }
    };
    let stdout = finish_output_reader(&mut stdout_task, command_name, "stdout").await;
    let stderr = finish_output_reader(&mut stderr_task, command_name, "stderr").await;

    if !status.success() {
        let message = if stderr.trim().is_empty() {
            format!("dump command exited with status {status}")
        } else {
            stderr
        };
        return Err(AppError::Internal(anyhow::anyhow!(message)));
    }

    Ok(stdout)
}

fn postgres_major_from_server_version_num(version_num: &str) -> AppResult<String> {
    let value = version_num.parse::<u32>().map_err(|_| {
        AppError::Internal(anyhow::anyhow!("failed to parse PostgreSQL server version"))
    })?;
    let major = value / 10000;
    if matches!(major, 14..=18) {
        Ok(major.to_string())
    } else {
        Err(AppError::Validation(format!(
            "unsupported PostgreSQL server version {major}; supported versions are 14 through 18"
        )))
    }
}

fn postgres_major_from_pg_dump_version(output: &str) -> AppResult<String> {
    for token in output.split_whitespace() {
        if token
            .chars()
            .next()
            .is_some_and(|char| char.is_ascii_digit())
        {
            let major = token
                .split('.')
                .next()
                .unwrap_or_default()
                .trim_matches(|c: char| !c.is_ascii_digit());
            if !major.is_empty() {
                return Ok(major.to_string());
            }
        }
    }

    Err(AppError::Internal(anyhow::anyhow!(
        "failed to parse pg_dump version"
    )))
}

async fn run_command(
    command_name: &str,
    mut command: Command,
    timeout_seconds: i32,
) -> AppResult<()> {
    configure_child_process(&mut command);
    let mut child = command
        .spawn()
        .map_err(|err| command_spawn_error(command_name, err))?;
    let stderr = child.stderr.take();
    let mut stderr_task = tokio::spawn(read_stderr(stderr));

    let status =
        match tokio::time::timeout(Duration::from_secs(timeout_seconds as u64), child.wait()).await
        {
            Ok(result) => result.map_err(|err| AppError::Internal(err.into()))?,
            Err(_) => {
                terminate_child_process(&mut child).await;
                stderr_task.abort();
                let _ = stderr_task.await;
                return Err(AppError::BackupTimeout);
            }
        };

    let stderr = match tokio::time::timeout(Duration::from_secs(1), &mut stderr_task).await {
        Ok(result) => result.unwrap_or_default(),
        Err(_) => {
            stderr_task.abort();
            let _ = stderr_task.await;
            tracing::warn!(
                command = command_name,
                "stderr pipe remained open after command exit"
            );
            String::new()
        }
    };
    if !status.success() {
        let message = if stderr.trim().is_empty() {
            format!("dump command exited with status {status}")
        } else {
            stderr
        };
        return Err(AppError::Internal(anyhow::anyhow!(message)));
    }

    Ok(())
}

fn configure_child_process(command: &mut Command) {
    command.kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;

        let parent_pid = unsafe { libc::getpid() };
        unsafe {
            command.as_std_mut().pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                    return Err(io::Error::last_os_error());
                }
                // Close the small fork/prctl race if the backend exited before the signal was set.
                if libc::getppid() != parent_pid {
                    libc::raise(libc::SIGKILL);
                }
                Ok(())
            });
        }
    }
}

fn command_spawn_error(command_name: &str, err: io::Error) -> AppError {
    if err.kind() == io::ErrorKind::NotFound {
        AppError::Internal(anyhow::anyhow!(
            "missing required dump command `{command_name}` in PATH"
        ))
    } else {
        AppError::Internal(anyhow::anyhow!("failed to spawn `{command_name}`: {err}"))
    }
}

async fn terminate_child_process(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(process_id) = child.id() {
        // Each command starts in its own process group, so a timeout also terminates descendants
        // that inherited stdout/stderr and could otherwise keep the worker hung indefinitely.
        unsafe {
            libc::kill(-(process_id as i32), libc::SIGKILL);
        }
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

async fn finish_output_reader(
    task: &mut tokio::task::JoinHandle<String>,
    command_name: &str,
    stream_name: &str,
) -> String {
    match tokio::time::timeout(Duration::from_secs(1), &mut *task).await {
        Ok(result) => result.unwrap_or_default(),
        Err(_) => {
            task.abort();
            tracing::warn!(
                command = command_name,
                stream = stream_name,
                "command output pipe remained open after process exit"
            );
            String::new()
        }
    }
}

async fn read_stderr(stderr: Option<tokio::process::ChildStderr>) -> String {
    read_stream(stderr).await
}

async fn read_stream<R>(stream: Option<R>) -> String
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let Some(stream) = stream else {
        return String::new();
    };
    let mut buffer = Vec::new();
    let mut limited = stream.take(64 * 1024);
    if limited.read_to_end(&mut buffer).await.is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&buffer).to_string()
}

async fn mark_success(
    pool: &PgPool,
    history_id: Uuid,
    file_name: &str,
    output_path: &Path,
    file_size: i64,
) -> AppResult<()> {
    let result = sqlx::query(
        r#"
        UPDATE backup_history
        SET status = 'success', file_name = $2, file_size = $3, file_path = $4,
            completed_at = now(), error_message = NULL
        WHERE id = $1 AND status = 'running'
        "#,
    )
    .bind(history_id)
    .bind(file_name)
    .bind(file_size)
    .bind(output_path.to_string_lossy().to_string())
    .execute(pool)
    .await?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(AppError::Conflict(
            "backup is no longer in a running state".into(),
        ))
    }
}

async fn mark_success_with_retry(
    pool: &PgPool,
    history_id: Uuid,
    file_name: &str,
    output_path: &Path,
    file_size: i64,
) -> AppResult<()> {
    let mut last_error = None;
    for attempt in 1..=3 {
        match mark_success(pool, history_id, file_name, output_path, file_size).await {
            Ok(()) => return Ok(()),
            Err(err) => {
                last_error = Some(err);
                if attempt < 3 {
                    tokio::time::sleep(Duration::from_millis(250 * attempt)).await;
                }
            }
        }
    }
    Err(last_error.expect("mark_success retry loop must record an error"))
}

async fn mark_failed(
    pool: &PgPool,
    history_id: Uuid,
    status: &str,
    error_message: &str,
    output_path: Option<&Path>,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE backup_history
        SET status = $2, error_message = $3, completed_at = now(), file_path = $4
        WHERE id = $1
        "#,
    )
    .bind(history_id)
    .bind(status)
    .bind(error_message)
    .bind(output_path.map(|path| path.to_string_lossy().to_string()))
    .execute(pool)
    .await?;
    Ok(())
}

async fn mark_failed_if_running(
    pool: &PgPool,
    history_id: Uuid,
    status: &str,
    error_message: &str,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE backup_history
        SET status = $2, error_message = $3, completed_at = now(),
            file_path = NULL, file_size = NULL
        WHERE id = $1 AND status = 'running'
        "#,
    )
    .bind(history_id)
    .bind(status)
    .bind(error_message)
    .execute(pool)
    .await?;
    Ok(())
}

async fn backup_event(pool: &PgPool, history_id: Uuid, stage: &str, level: &str, message: &str) {
    if let Err(err) = sqlx::query(
        r#"
        INSERT INTO backup_events (id, history_id, stage, level, message)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(history_id)
    .bind(stage)
    .bind(level)
    .bind(message)
    .execute(pool)
    .await
    {
        tracing::warn!(history_id = %history_id, stage = %stage, error = ?err, "failed to write backup event");
    }
}

const RETENTION_SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Applies retention at startup and hourly, including disabled configs or configs whose recent
/// backups keep failing and therefore never reach the normal post-success retention step.
pub fn spawn_retention_worker(runtime: BackupRuntime) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(RETENTION_SWEEP_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            match sweep_all_retention(&runtime).await {
                Ok((checked, failed)) => {
                    tracing::info!(checked, failed, "periodic backup retention sweep completed");
                }
                Err(err) => {
                    tracing::warn!(error = ?err, "periodic backup retention sweep failed");
                }
            }
        }
    })
}

async fn sweep_all_retention(runtime: &BackupRuntime) -> AppResult<(u64, u64)> {
    let config_ids = sqlx::query_scalar::<_, Uuid>("SELECT id FROM backup_configs ORDER BY id")
        .fetch_all(&runtime.db)
        .await?;

    let checked = config_ids.len() as u64;
    let mut failed = 0_u64;
    for config_id in config_ids {
        if let Err(err) = apply_retention(runtime, config_id).await {
            failed += 1;
            tracing::warn!(%config_id, error = ?err, "retention sweep failed for config");
        }
    }
    Ok((checked, failed))
}

async fn apply_retention(runtime: &BackupRuntime, config_id: Uuid) -> AppResult<()> {
    let mut transaction = runtime.db.begin().await?;
    // Serialize with config updates/deletion, then read the current policy. This prevents a
    // worker that captured an older, stricter policy from deleting files after the user relaxed
    // retention. File removal itself is delegated to the durable outbox after commit.
    let settings = sqlx::query_as::<_, (i32, Option<i32>)>(
        r#"
        SELECT retention_days, max_backups
        FROM backup_configs
        WHERE id = $1
        FOR UPDATE
        "#,
    )
    .bind(config_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some((retention_days, max_backups)) = settings else {
        transaction.rollback().await?;
        return Ok(());
    };

    let queued = sqlx::query(
        r#"
        WITH ranked AS (
            SELECT id, file_path, completed_at,
                   row_number() OVER (
                       ORDER BY completed_at DESC NULLS LAST, started_at DESC, id DESC
                   ) AS backup_rank
            FROM backup_history
            WHERE config_id = $1
              AND status = 'success'
              AND file_path IS NOT NULL
        ),
        candidates AS (
            SELECT id, file_path
            FROM ranked
            WHERE completed_at < now() - ($2::int * interval '1 day')
               OR ($3::int IS NOT NULL AND backup_rank > $3::bigint)
        ),
        enqueued AS (
            INSERT INTO pending_file_deletions (id, file_path)
            SELECT gen_random_uuid(), file_path
            FROM candidates
            ON CONFLICT (file_path) DO NOTHING
            RETURNING file_path
        )
        UPDATE backup_history AS history
        SET file_path = NULL
        FROM candidates
        WHERE history.id = candidates.id
          AND history.file_path = candidates.file_path
        "#,
    )
    .bind(config_id)
    .bind(retention_days)
    .bind(max_backups)
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    transaction.commit().await?;

    if queued > 0 {
        tracing::info!(%config_id, queued, "retained backup files queued for durable deletion");
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManagedFileRemoval {
    Removed,
    Missing,
    OutsideBackupDir,
    NotAFile,
}

/// Removes only a direct child of the managed backup directory without following a final symlink.
pub(crate) async fn remove_managed_file(
    backup_dir: &Path,
    path: &Path,
) -> std::io::Result<ManagedFileRemoval> {
    let metadata = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(ManagedFileRemoval::Missing)
        }
        Err(err) => return Err(err),
    };
    let backup_dir = tokio::fs::canonicalize(backup_dir).await?;
    let Some(parent) = path.parent() else {
        return Ok(ManagedFileRemoval::OutsideBackupDir);
    };
    let parent = tokio::fs::canonicalize(parent).await?;
    if parent != backup_dir {
        return Ok(ManagedFileRemoval::OutsideBackupDir);
    }
    let file_type = metadata.file_type();
    if !file_type.is_file() && !file_type.is_symlink() {
        return Ok(ManagedFileRemoval::NotAFile);
    }
    tokio::fs::remove_file(path).await?;
    Ok(ManagedFileRemoval::Removed)
}

#[derive(Debug, PartialEq, Eq)]
struct DurableFinalOutput {
    path: PathBuf,
    size: i64,
}

/// Returns a completed output only when the recorded final name resolves to a direct regular
/// child of the backup directory. `symlink_metadata` deliberately avoids following a final
/// symlink: a symlink, directory, unsafe name, or missing path can never commit a run.
async fn find_durable_final_output(
    backup_dir: &Path,
    backup: &InterruptedBackup,
) -> std::io::Result<Option<DurableFinalOutput>> {
    let Some(file_name) = backup.file_name.as_deref() else {
        return Ok(None);
    };
    if !is_safe_final_file_name(file_name) {
        tracing::warn!(
            history_id = %backup.id,
            file_name,
            "interrupted backup has an unsafe final file name"
        );
        return Ok(None);
    }

    let path = backup_dir.join(file_name);
    let metadata = match tokio::fs::symlink_metadata(&path).await {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    if !metadata.file_type().is_file() {
        tracing::warn!(
            history_id = %backup.id,
            path = %path.display(),
            "interrupted backup final path is not a direct regular file"
        );
        return Ok(None);
    }

    let canonical_backup_dir = tokio::fs::canonicalize(backup_dir).await?;
    let Some(parent) = path.parent() else {
        return Ok(None);
    };
    if tokio::fs::canonicalize(parent).await? != canonical_backup_dir {
        tracing::warn!(
            history_id = %backup.id,
            path = %path.display(),
            "interrupted backup final path is outside the backup directory"
        );
        return Ok(None);
    }

    let size = i64::try_from(metadata.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("backup file is too large to record: {}", path.display()),
        )
    })?;
    Ok(Some(DurableFinalOutput { path, size }))
}

async fn cleanup_interrupted_partial(
    backup_dir: &Path,
    backup: &InterruptedBackup,
) -> std::io::Result<()> {
    if let Some(partial_path) = backup.file_path.as_deref().map(Path::new) {
        if partial_path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with(".part"))
        {
            remove_recovery_file(backup_dir, partial_path).await?;
        }
    }
    Ok(())
}

fn is_safe_final_file_name(file_name: &str) -> bool {
    let mut components = Path::new(file_name).components();
    matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
        && file_name.ends_with(".sql")
}

async fn remove_recovery_file(backup_dir: &Path, path: &Path) -> std::io::Result<()> {
    match remove_managed_file(backup_dir, path).await? {
        ManagedFileRemoval::Removed | ManagedFileRemoval::Missing => {}
        ManagedFileRemoval::OutsideBackupDir => {
            tracing::warn!(path = %path.display(), "skipped interrupted file outside backup dir")
        }
        ManagedFileRemoval::NotAFile => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("interrupted backup path is not a file: {}", path.display()),
            ));
        }
    }
    Ok(())
}

async fn sweep_partial_outputs(backup_dir: &Path) {
    let mut entries = match tokio::fs::read_dir(backup_dir).await {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!(path = %backup_dir.display(), error = ?err, "failed to scan partial backup files");
            return;
        }
    };

    loop {
        let entry = match entries.next_entry().await {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            Err(err) => {
                tracing::warn!(path = %backup_dir.display(), error = ?err, "failed to read partial backup file entry");
                break;
            }
        };
        if !entry.file_name().to_string_lossy().ends_with(".part") {
            continue;
        }
        match entry.file_type().await {
            Ok(file_type) if file_type.is_file() || file_type.is_symlink() => {
                cleanup_partial_file(&entry.path()).await
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(path = %entry.path().display(), error = ?err, "failed to inspect partial backup file");
            }
        }
    }
}

async fn cleanup_partial_file(path: &Path) {
    if let Err(err) = tokio::fs::remove_file(path).await {
        if err.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(path = %path.display(), error = ?err, "failed to remove partial backup file");
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct InterruptedBackup {
    id: Uuid,
    file_name: Option<String>,
    file_path: Option<String>,
}

#[derive(Debug)]
struct DumpTarget {
    scheme: String,
    host: String,
    port: u16,
    username: String,
    password: Option<String>,
    database: String,
}

impl DumpTarget {
    fn parse(input: &str) -> AppResult<Self> {
        let url = url::Url::parse(input)
            .map_err(|_| AppError::Validation("db_url must be a valid URL".into()))?;
        let host = url
            .host_str()
            .ok_or_else(|| AppError::Validation("db_url host is required".into()))?
            .to_string();
        let username = decode_url_component(url.username());
        if username.is_empty() {
            return Err(AppError::Validation("db_url username is required".into()));
        }
        let database = decode_url_component(url.path().trim_start_matches('/'));
        if database.is_empty() {
            return Err(AppError::Validation(
                "db_url database name is required".into(),
            ));
        }
        if url.scheme() == "mysql" && database.starts_with('-') {
            return Err(AppError::Validation(
                "MySQL database name must not start with '-'".into(),
            ));
        }

        let default_port = match url.scheme() {
            "postgres" | "postgresql" => 5432,
            "mysql" => 3306,
            _ => return Err(AppError::Validation("unsupported db_url scheme".into())),
        };

        let target = Self {
            scheme: url.scheme().to_string(),
            host,
            port: url.port().unwrap_or(default_port),
            username,
            password: url.password().map(decode_url_component),
            database,
        };
        target.validate_no_newlines()?;
        Ok(target)
    }

    fn validate_db_type(&self, db_type: &str) -> AppResult<()> {
        let valid = match db_type {
            "postgres" => matches!(self.scheme.as_str(), "postgres" | "postgresql"),
            "mysql" => self.scheme == "mysql",
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(AppError::Validation(
                "db_url scheme must match db_type".into(),
            ))
        }
    }

    fn validate_no_newlines(&self) -> AppResult<()> {
        let values = [
            self.host.as_str(),
            self.username.as_str(),
            self.password.as_deref().unwrap_or_default(),
            self.database.as_str(),
        ];
        if values
            .iter()
            .any(|value| value.chars().any(char::is_control))
        {
            return Err(AppError::Validation(
                "db_url components must not contain control characters".into(),
            ));
        }
        Ok(())
    }
}

pub async fn detect_db_version(
    db_type: &str,
    db_url: &str,
    timeout_seconds: i32,
) -> AppResult<String> {
    let target = DumpTarget::parse(db_url)?;
    target.validate_db_type(db_type)?;
    match db_type {
        "postgres" => detect_postgres_major_version(&target, timeout_seconds).await,
        "mysql" => {
            let version = detect_mysql_version(&target, timeout_seconds).await?;
            let parts: Vec<&str> = version.split('.').collect();
            if parts.len() >= 2 {
                Ok(format!("{}.{}", parts[0], parts[1]))
            } else {
                Ok(version)
            }
        }
        other => Err(AppError::Validation(format!("unsupported db_type {other}"))),
    }
}

pub fn validate_database_url(db_type: &str, input: &str) -> AppResult<()> {
    let target = DumpTarget::parse(input)?;
    target.validate_db_type(db_type)
}

fn decode_url_component(value: &str) -> String {
    percent_decode_str(value).decode_utf8_lossy().to_string()
}

fn mysql_option_file(target: &DumpTarget) -> AppResult<NamedTempFile> {
    let mut file = NamedTempFile::new().map_err(|err| AppError::Internal(err.into()))?;
    let contents = format!(
        "[client]\nhost={}\nport={}\nuser={}\npassword={}\n",
        mysql_option_value(&target.host),
        target.port,
        mysql_option_value(&target.username),
        mysql_option_value(target.password.as_deref().unwrap_or_default())
    );
    std::io::Write::write_all(&mut file, contents.as_bytes())
        .map_err(|err| AppError::Internal(err.into()))?;
    Ok(file)
}

fn mysql_option_value(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            _ => escaped.push(character),
        }
    }
    escaped.push('"');
    escaped
}

#[cfg(test)]
mod tests;
