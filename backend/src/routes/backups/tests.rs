use super::*;
use crate::services::file_deletion;

fn valid_payload() -> BackupConfigRequest {
    BackupConfigRequest {
        name: "Daily production backup".into(),
        db_type: "postgres".into(),
        db_version: Some("16".into()),
        db_url: Some("postgres://user:secret@localhost/app".into()),
        cron_schedule: Some("0 0 2 * * *".into()),
        retention_days: Some(14),
        timeout_seconds: Some(300),
        max_backups: Some(7),
        is_enabled: Some(true),
        email_to: Vec::new(),
        email_cc: Vec::new(),
        email_notify_on: Some("never".into()),
    }
}

#[test]
fn validate_payload_accepts_valid_config() {
    assert!(validate_payload(&valid_payload()).is_ok());
}

#[test]
fn validate_payload_rejects_invalid_required_fields() {
    let mut payload = valid_payload();
    payload.name = "   ".into();
    assert!(matches!(
        validate_payload(&payload),
        Err(AppError::Validation(message)) if message == "name is required"
    ));

    let mut payload = valid_payload();
    payload.db_type = "sqlite".into();
    assert!(matches!(
        validate_payload(&payload),
        Err(AppError::Validation(message)) if message == "db_type must be postgres or mysql"
    ));
}

#[test]
fn validate_payload_rejects_invalid_limits_and_versions() {
    let mut payload = valid_payload();
    payload.retention_days = Some(0);
    assert!(matches!(
        validate_payload(&payload),
        Err(AppError::Validation(message)) if message == "retention_days must be positive"
    ));

    let mut payload = valid_payload();
    payload.timeout_seconds = Some(0);
    assert!(matches!(
        validate_payload(&payload),
        Err(AppError::Validation(message)) if message == "timeout_seconds must be positive"
    ));

    let mut payload = valid_payload();
    payload.max_backups = Some(0);
    assert!(matches!(
        validate_payload(&payload),
        Err(AppError::Validation(message)) if message == "max_backups must be positive"
    ));

    let mut payload = valid_payload();
    payload.db_version = Some("13".into());
    assert!(matches!(
        validate_payload(&payload),
        Err(AppError::Validation(message))
            if message == "db_version must be auto, 14, 15, 16, 17, or 18"
    ));
}

#[test]
fn validate_payload_rejects_invalid_email_notification_settings() {
    let mut payload = valid_payload();
    payload.email_notify_on = Some("sometimes".into());
    assert!(matches!(
        validate_payload(&payload),
        Err(AppError::Validation(message))
            if message == "email_notify_on must be never, failure, or always"
    ));

    let mut payload = valid_payload();
    payload.email_notify_on = Some("failure".into());
    assert!(matches!(
        validate_payload(&payload),
        Err(AppError::Validation(message))
            if message == "email_to is required when email notifications are enabled"
    ));

    let mut payload = valid_payload();
    payload.email_to = vec!["not-an-email".into()];
    assert!(matches!(
        validate_payload(&payload),
        Err(AppError::Validation(message)) if message == "invalid email address `not-an-email`"
    ));
}

#[test]
fn email_lists_are_trimmed_lowercased_and_deduplicated() {
    let emails = normalize_email_list(&[
        " Ops@Example.COM ".into(),
        "ops@example.com".into(),
        "".into(),
        "Admin@Example.com".into(),
    ]);

    assert_eq!(emails, vec!["ops@example.com", "admin@example.com"]);
}

#[test]
fn normalize_db_version_treats_auto_and_blank_as_detection_mode() {
    assert_eq!(normalize_db_version(Some(" 17 ".into())), Some("17".into()));
    assert_eq!(normalize_db_version(Some("auto".into())), None);
    assert_eq!(normalize_db_version(Some("   ".into())), None);
    assert_eq!(normalize_db_version(None), None);
}

async fn integration_pool() -> (sqlx::PgPool, String) {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point to a disposable PostgreSQL database");
    let pool = crate::db::connect(&database_url)
        .await
        .expect("test database should be reachable");
    crate::db::migrate(&pool)
        .await
        .expect("test database migrations should succeed");
    (pool, database_url)
}

async fn insert_test_config(pool: &sqlx::PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO backup_configs
            (id, name, db_type, db_url_encrypted, db_url_nonce)
        VALUES ($1, $2, 'postgres', 'test-ciphertext', 'test-nonce')
        "#,
    )
    .bind(id)
    .bind(format!("delete-trigger-lock-test-{id}"))
    .execute(pool)
    .await
    .expect("test config should be inserted");
    id
}

async fn insert_success_history(pool: &sqlx::PgPool, config_id: Uuid, file_path: &str) {
    sqlx::query(
        r#"
        INSERT INTO backup_history
            (id, config_id, status, file_name, file_size, file_path,
             started_at, completed_at, triggered_by)
        VALUES ($1, $2, 'success', 'test.sql', 4, $3, now(), now(), 'manual')
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(config_id)
    .bind(file_path)
    .execute(pool)
    .await
    .expect("test history should be inserted");
}

fn test_backup_runtime(pool: sqlx::PgPool, database_url: String) -> backup_executor::BackupRuntime {
    let config = crate::config::AppConfig {
        database_url,
        jwt_secret: "test-jwt-secret".into(),
        database_encryption_key: "test-encryption-key".into(),
        previous_database_encryption_key: None,
        backup_dir: std::env::temp_dir().join("dumply-delete-trigger-lock-test"),
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        admin_username: "test-admin".into(),
        admin_password: "test-password".into(),
        reset_admin_password_on_start: false,
        jwt_ttl_seconds: 60,
        max_concurrent_backups: 1,
        cors_allowed_origin: None,
        smtp: None,
    };
    backup_executor::BackupRuntime {
        db: pool,
        config: std::sync::Arc::new(config),
        permits: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
    }
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to a disposable PostgreSQL database"]
async fn parent_row_lock_serializes_delete_and_trigger() {
    let (pool, database_url) = integration_pool().await;

    // Trigger wins: an uncommitted running history must make deletion wait and then conflict.
    let trigger_first_id = insert_test_config(&pool).await;
    let mut trigger_transaction = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM backup_configs WHERE id = $1 FOR UPDATE")
        .bind(trigger_first_id)
        .fetch_one(&mut *trigger_transaction)
        .await
        .unwrap();
    sqlx::query(
        r#"
        INSERT INTO backup_history (id, config_id, status, started_at, triggered_by)
        VALUES ($1, $2, 'running', now(), 'manual')
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(trigger_first_id)
    .execute(&mut *trigger_transaction)
    .await
    .unwrap();

    let delete_pool = pool.clone();
    let mut delete_task =
        tokio::spawn(async move { delete_config_records(&delete_pool, trigger_first_id).await });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut delete_task)
            .await
            .is_err(),
        "delete should wait for the trigger transaction's parent-row lock"
    );
    trigger_transaction.commit().await.unwrap();
    assert!(matches!(
        delete_task.await.unwrap(),
        Err(AppError::Conflict(message))
            if message == "cannot delete config while backup is running"
    ));
    sqlx::query("DELETE FROM backup_configs WHERE id = $1")
        .bind(trigger_first_id)
        .execute(&pool)
        .await
        .unwrap();

    // Delete wins: a trigger waiting on the same row must observe that it was deleted.
    let delete_first_id = insert_test_config(&pool).await;
    let mut delete_transaction = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM backup_configs WHERE id = $1 FOR UPDATE")
        .bind(delete_first_id)
        .fetch_one(&mut *delete_transaction)
        .await
        .unwrap();

    let runtime = test_backup_runtime(pool.clone(), database_url);
    let mut trigger_task = tokio::spawn(async move {
        backup_executor::spawn_backup(runtime, delete_first_id, BackupTrigger::Manual).await
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut trigger_task)
            .await
            .is_err(),
        "trigger should wait for the delete transaction's parent-row lock"
    );
    sqlx::query("DELETE FROM backup_configs WHERE id = $1")
        .bind(delete_first_id)
        .execute(&mut *delete_transaction)
        .await
        .unwrap();
    delete_transaction.commit().await.unwrap();
    assert!(matches!(
        trigger_task.await.unwrap(),
        Err(AppError::NotFound)
    ));
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to a disposable PostgreSQL database"]
async fn config_delete_uses_a_durable_file_deletion_outbox() {
    let (pool, _) = integration_pool().await;
    let backup_directory = tempfile::tempdir().unwrap();

    let removable_config = insert_test_config(&pool).await;
    let removable_path = backup_directory
        .path()
        .join(format!("{removable_config}.sql"));
    std::fs::write(&removable_path, b"dump").unwrap();
    let removable_path_text = removable_path.to_string_lossy().to_string();
    insert_success_history(&pool, removable_config, &removable_path_text).await;

    delete_config_records(&pool, removable_config)
        .await
        .unwrap();

    assert!(removable_path.exists());
    let config_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM backup_configs WHERE id = $1)")
            .bind(removable_config)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!config_exists);
    let queued: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pending_file_deletions WHERE file_path = $1")
            .bind(&removable_path_text)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(queued, 1);
    file_deletion::purge_pending_file_deletions(&pool, backup_directory.path())
        .await
        .unwrap();
    assert!(!removable_path.exists());
    let queued: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pending_file_deletions WHERE file_path = $1")
            .bind(&removable_path_text)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(queued, 0);

    let retained_config = insert_test_config(&pool).await;
    let outside_directory = tempfile::tempdir().unwrap();
    let retained_path = outside_directory
        .path()
        .join(format!("{retained_config}.sql"));
    std::fs::write(&retained_path, b"dump").unwrap();
    let retained_path_text = retained_path.to_string_lossy().to_string();
    insert_success_history(&pool, retained_config, &retained_path_text).await;

    delete_config_records(&pool, retained_config).await.unwrap();

    file_deletion::purge_pending_file_deletions(&pool, backup_directory.path())
        .await
        .unwrap();

    assert!(retained_path.exists());
    let config_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM backup_configs WHERE id = $1)")
            .bind(retained_config)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!config_exists);
    let (attempts, last_error, attempted): (i32, Option<String>, bool) = sqlx::query_as(
        r#"
        SELECT attempts, last_error, last_attempt_at IS NOT NULL
        FROM pending_file_deletions
        WHERE file_path = $1
        "#,
    )
    .bind(&retained_path_text)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(attempts, 1);
    assert!(last_error
        .as_deref()
        .is_some_and(|error| error.contains("outside")));
    assert!(attempted);

    std::fs::remove_file(&retained_path).unwrap();
    sqlx::query("UPDATE pending_file_deletions SET next_attempt_at = now() WHERE file_path = $1")
        .bind(&retained_path_text)
        .execute(&pool)
        .await
        .unwrap();
    file_deletion::purge_pending_file_deletions(&pool, backup_directory.path())
        .await
        .unwrap();
    let queued: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pending_file_deletions WHERE file_path = $1")
            .bind(&retained_path_text)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(queued, 0);
}
