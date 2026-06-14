use lettre::{
    message::Mailbox, transport::smtp::authentication::Credentials, AsyncSmtpTransport,
    AsyncTransport, Message, Tokio1Executor,
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    config::{AppConfig, SmtpTlsMode},
    db::models::BackupConfig,
    error::{AppError, AppResult},
};

#[derive(Debug, sqlx::FromRow)]
struct NotificationHistory {
    status: String,
    file_name: Option<String>,
    file_size: Option<i64>,
    error_message: Option<String>,
    started_at: chrono::DateTime<chrono::Utc>,
    completed_at: Option<chrono::DateTime<chrono::Utc>>,
    triggered_by: String,
}

pub async fn notify_backup_completed(
    pool: &PgPool,
    app_config: &AppConfig,
    backup_config: &BackupConfig,
    history_id: Uuid,
    status: &str,
) {
    if !should_notify(backup_config.email_notify_on.as_str(), status) {
        return;
    }
    if backup_config.email_to.is_empty() {
        notification_event(
            pool,
            history_id,
            "warning",
            "Email notification skipped because no recipients are configured.",
        )
        .await;
        return;
    }

    let Some(smtp) = &app_config.smtp else {
        notification_event(
            pool,
            history_id,
            "warning",
            "Email notification skipped because SMTP is not configured.",
        )
        .await;
        return;
    };

    match send_notification(pool, smtp, backup_config, history_id).await {
        Ok(()) => {
            notification_event(pool, history_id, "success", "Email notification sent.").await;
        }
        Err(err) => {
            tracing::warn!(history_id = %history_id, error = ?err, "email notification failed");
            notification_event(
                pool,
                history_id,
                "warning",
                &format!("Email notification failed: {err}"),
            )
            .await;
        }
    }
}

fn should_notify(rule: &str, status: &str) -> bool {
    match rule {
        "always" => matches!(status, "success" | "failed" | "timeout"),
        "failure" => matches!(status, "failed" | "timeout"),
        _ => false,
    }
}

async fn send_notification(
    pool: &PgPool,
    smtp: &crate::config::SmtpConfig,
    backup_config: &BackupConfig,
    history_id: Uuid,
) -> AppResult<()> {
    let history = sqlx::query_as::<_, NotificationHistory>(
        r#"
        SELECT status, file_name, file_size, error_message, started_at, completed_at, triggered_by
        FROM backup_history
        WHERE id = $1
        "#,
    )
    .bind(history_id)
    .fetch_one(pool)
    .await?;

    let mut builder = Message::builder()
        .from(parse_mailbox(&smtp.from)?)
        .subject(format!(
            "[DumplySquirrel] Backup {}: {}",
            history.status, backup_config.name
        ));

    for recipient in &backup_config.email_to {
        builder = builder.to(parse_mailbox(recipient)?);
    }
    for recipient in &backup_config.email_cc {
        builder = builder.cc(parse_mailbox(recipient)?);
    }

    let message = builder
        .body(email_body(backup_config, history_id, &history))
        .map_err(|err| AppError::Internal(err.into()))?;
    let mut transport_builder = match smtp.tls {
        SmtpTlsMode::StartTls => AsyncSmtpTransport::<Tokio1Executor>::relay(&smtp.host)
            .map_err(|err| AppError::Internal(err.into()))?,
        SmtpTlsMode::None => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&smtp.host),
    }
    .port(smtp.port);

    if let Some(username) = &smtp.username {
        transport_builder = transport_builder.credentials(Credentials::new(
            username.clone(),
            smtp.password.clone().unwrap_or_default(),
        ));
    }

    transport_builder
        .build()
        .send(message)
        .await
        .map_err(|err| AppError::Internal(err.into()))?;
    Ok(())
}

fn parse_mailbox(value: &str) -> AppResult<Mailbox> {
    value
        .parse::<Mailbox>()
        .map_err(|err| AppError::Validation(format!("invalid email address `{value}`: {err}")))
}

fn email_body(config: &BackupConfig, history_id: Uuid, history: &NotificationHistory) -> String {
    let mut lines = vec![
        format!("Backup task: {}", config.name),
        format!("Status: {}", history.status),
        format!("Trigger: {}", history.triggered_by),
        format!("History ID: {history_id}"),
        format!("Started at: {}", history.started_at),
    ];

    if let Some(completed_at) = history.completed_at {
        lines.push(format!("Completed at: {completed_at}"));
    }
    if let Some(file_name) = &history.file_name {
        lines.push(format!("File: {file_name}"));
    }
    if let Some(file_size) = history.file_size {
        lines.push(format!("File size: {file_size} bytes"));
    }
    if let Some(error) = &history.error_message {
        lines.push(String::new());
        lines.push("Error:".into());
        lines.push(error.clone());
    }

    lines.join("\n")
}

async fn notification_event(pool: &PgPool, history_id: Uuid, level: &str, message: &str) {
    if let Err(err) = sqlx::query(
        r#"
        INSERT INTO backup_events (id, history_id, stage, level, message)
        VALUES ($1, $2, 'notify', $3, $4)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(history_id)
    .bind(level)
    .bind(message)
    .execute(pool)
    .await
    {
        tracing::warn!(history_id = %history_id, error = ?err, "failed to write email notification event");
    }
}
