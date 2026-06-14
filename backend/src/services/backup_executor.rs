use std::{fs::File, io, path::Path, process::Stdio, sync::Arc, time::Duration};

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
    let config = load_config(&runtime.db, config_id).await?;
    let history = create_running_history(&runtime.db, config_id, trigger).await?;
    let history_id = history.id;

    tokio::spawn(async move {
        if let Err(err) = run_backup(runtime, config, history_id).await {
            tracing::error!(history_id = %history_id, error = ?err, "backup worker failed");
        }
    });

    Ok(history)
}

async fn load_config(pool: &PgPool, config_id: Uuid) -> AppResult<BackupConfig> {
    Ok(sqlx::query_as::<_, BackupConfig>(
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
    .fetch_one(pool)
    .await?)
}

async fn create_running_history(
    pool: &PgPool,
    config_id: Uuid,
    trigger: BackupTrigger,
) -> AppResult<BackupHistory> {
    let result = sqlx::query_as::<_, BackupHistory>(
        r#"
        INSERT INTO backup_history (id, config_id, status, started_at, triggered_by)
        VALUES ($1, $2, 'running', $3, $4)
        RETURNING id, config_id, status, file_name, file_size, file_path,
                  error_message, started_at, completed_at, triggered_by
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(config_id)
    .bind(Utc::now())
    .bind(trigger.as_str())
    .fetch_one(pool)
    .await;

    match result {
        Ok(history) => Ok(history),
        Err(sqlx::Error::Database(err)) if err.code().as_deref() == Some("23505") => Err(
            AppError::Conflict("backup is already running for this config".into()),
        ),
        Err(err) => Err(err.into()),
    }
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
    let file_name = format!("{}_{}.sql", config.id, Utc::now().format("%Y%m%d_%H%M%S"));
    let output_path = runtime.config.backup_dir.join(&file_name);
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
                &output_path,
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
                &output_path,
                config.db_version.as_deref(),
                config.timeout_seconds,
            )
            .await
        }
        other => Err(AppError::Validation(format!("unsupported db_type {other}"))),
    };

    match dump_result {
        Ok(()) => {
            let file_size = tokio::fs::metadata(&output_path)
                .await
                .map_err(|err| AppError::Internal(err.into()))?
                .len() as i64;
            backup_event(
                &runtime.db,
                history_id,
                "seal",
                "success",
                &format!("Backup file sealed at {file_size} bytes."),
            )
            .await;
            mark_success(&runtime.db, history_id, &file_name, &output_path, file_size).await?;
            backup_event(
                &runtime.db,
                history_id,
                "retention",
                "info",
                "Applying retention rules.",
            )
            .await;
            apply_retention(runtime, config).await?;
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
            cleanup_partial_file(&output_path).await;
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
            cleanup_partial_file(&output_path).await;
            backup_event(&runtime.db, history_id, "failed", "error", &err.to_string()).await;
            mark_failed(&runtime.db, history_id, "failed", &err.to_string(), None).await?;
            Err(err)
        }
    }
}

async fn run_pg_dump(
    pool: &PgPool,
    history_id: Uuid,
    target: &DumpTarget,
    output_path: &Path,
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
    let output = File::create(output_path).map_err(|err| AppError::Internal(err.into()))?;
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
    output_path: &Path,
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
    let output = File::create(output_path).map_err(|err| AppError::Internal(err.into()))?;
    let option_path = option_file.path().to_path_buf();
    let mut command = Command::new("mysqldump");
    command
        .arg(format!("--defaults-extra-file={}", option_path.display()))
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
    let output = match tokio::time::timeout(
        Duration::from_secs(timeout_seconds.max(1) as u64),
        command.output(),
    )
    .await
    {
        Ok(result) => result.map_err(|err| {
            if err.kind() == io::ErrorKind::NotFound {
                AppError::Internal(anyhow::anyhow!(
                    "missing required dump command `{command_name}` in PATH"
                ))
            } else {
                AppError::Internal(anyhow::anyhow!("failed to run `{command_name}`: {err}"))
            }
        })?,
        Err(_) => return Err(AppError::BackupTimeout),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let message = if stderr.trim().is_empty() {
            format!("dump command exited with status {}", output.status)
        } else {
            stderr
        };
        return Err(AppError::Internal(anyhow::anyhow!(message)));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
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
    let mut child = command.spawn().map_err(|err| {
        if err.kind() == io::ErrorKind::NotFound {
            AppError::Internal(anyhow::anyhow!(
                "missing required dump command `{command_name}` in PATH"
            ))
        } else {
            AppError::Internal(anyhow::anyhow!("failed to spawn `{command_name}`: {err}"))
        }
    })?;
    let stderr = child.stderr.take();
    let stderr_task = tokio::spawn(read_stderr(stderr));

    let status =
        match tokio::time::timeout(Duration::from_secs(timeout_seconds as u64), child.wait()).await
        {
            Ok(result) => result.map_err(|err| AppError::Internal(err.into()))?,
            Err(_) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                let _ = stderr_task.await;
                return Err(AppError::BackupTimeout);
            }
        };

    let stderr = stderr_task.await.unwrap_or_default();
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

async fn read_stderr(stderr: Option<tokio::process::ChildStderr>) -> String {
    let Some(stderr) = stderr else {
        return String::new();
    };
    let mut buffer = Vec::new();
    let mut limited = stderr.take(64 * 1024);
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
    sqlx::query(
        r#"
        UPDATE backup_history
        SET status = 'success', file_name = $2, file_size = $3, file_path = $4,
            completed_at = now(), error_message = NULL
        WHERE id = $1
        "#,
    )
    .bind(history_id)
    .bind(file_name)
    .bind(file_size)
    .bind(output_path.to_string_lossy().to_string())
    .execute(pool)
    .await?;
    Ok(())
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
        SET status = $2, error_message = $3, completed_at = now()
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

async fn apply_retention(runtime: &BackupRuntime, config: &BackupConfig) -> AppResult<()> {
    let expired = sqlx::query_as::<_, RetentionCandidate>(
        r#"
        SELECT id, file_path
        FROM backup_history
        WHERE config_id = $1
          AND status = 'success'
          AND file_path IS NOT NULL
          AND completed_at < now() - ($2::int * interval '1 day')
        "#,
    )
    .bind(config.id)
    .bind(config.retention_days)
    .fetch_all(&runtime.db)
    .await?;
    remove_retained_files(runtime, expired).await?;

    if let Some(max_backups) = config.max_backups {
        let overflow = sqlx::query_as::<_, RetentionCandidate>(
            r#"
            SELECT id, file_path
            FROM backup_history
            WHERE config_id = $1
              AND status = 'success'
              AND file_path IS NOT NULL
            ORDER BY completed_at DESC NULLS LAST
            OFFSET $2
            "#,
        )
        .bind(config.id)
        .bind(max_backups as i64)
        .fetch_all(&runtime.db)
        .await?;
        remove_retained_files(runtime, overflow).await?;
    }

    Ok(())
}

async fn remove_retained_files(
    runtime: &BackupRuntime,
    candidates: Vec<RetentionCandidate>,
) -> AppResult<()> {
    for candidate in candidates {
        if let Some(file_path) = candidate.file_path {
            match is_under_backup_dir(&runtime.config.backup_dir, Path::new(&file_path)).await {
                Ok(true) => {
                    if let Err(err) = tokio::fs::remove_file(&file_path).await {
                        if err.kind() != std::io::ErrorKind::NotFound {
                            tracing::warn!(path = %file_path, error = ?err, "failed to remove retained backup file");
                            continue;
                        }
                    }
                    sqlx::query("UPDATE backup_history SET file_path = NULL WHERE id = $1")
                        .bind(candidate.id)
                        .execute(&runtime.db)
                        .await?;
                }
                Ok(false) => {
                    tracing::warn!(path = %file_path, "skipped retention file outside backup dir")
                }
                Err(err) => {
                    tracing::warn!(path = %file_path, error = ?err, "failed retention path check")
                }
            }
        }
    }
    Ok(())
}

async fn is_under_backup_dir(backup_dir: &Path, path: &Path) -> std::io::Result<bool> {
    let backup_dir = tokio::fs::canonicalize(backup_dir).await?;
    let path = tokio::fs::canonicalize(path).await?;
    Ok(path.starts_with(backup_dir))
}

async fn cleanup_partial_file(path: &Path) {
    if let Err(err) = tokio::fs::remove_file(path).await {
        if err.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(path = %path.display(), error = ?err, "failed to remove partial backup file");
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct RetentionCandidate {
    id: Uuid,
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
        if values.iter().any(|value| value.contains(['\n', '\r'])) {
            return Err(AppError::Validation(
                "db_url components must not contain newlines".into(),
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
        target.host,
        target.port,
        target.username,
        target.password.as_deref().unwrap_or_default()
    );
    std::io::Write::write_all(&mut file, contents.as_bytes())
        .map_err(|err| AppError::Internal(err.into()))?;
    Ok(file)
}

#[cfg(test)]
mod tests;
