use super::*;

#[test]
fn validates_matching_database_url_scheme() {
    assert!(validate_database_url("postgres", "postgres://user:pass@localhost/app").is_ok());
    assert!(validate_database_url("mysql", "mysql://user:pass@localhost/app").is_ok());
    assert!(validate_database_url("mysql", "postgres://user:pass@localhost/app").is_err());
}

#[test]
fn rejects_newlines_in_database_url_components() {
    assert!(validate_database_url("postgres", "postgres://user%0A:pass@localhost/app").is_err());
}

#[test]
fn rejects_mysql_database_names_that_could_be_parsed_as_options() {
    assert!(validate_database_url("mysql", "mysql://user:pass@localhost/--all-databases").is_err());
}

#[test]
fn parses_supported_postgres_server_versions() {
    assert_eq!(
        postgres_major_from_server_version_num("160002").unwrap(),
        "16"
    );
    assert_eq!(
        postgres_major_from_server_version_num("180000").unwrap(),
        "18"
    );
    assert!(postgres_major_from_server_version_num("130000").is_err());
}

#[test]
fn parses_pg_dump_major_version() {
    assert_eq!(
        postgres_major_from_pg_dump_version("pg_dump (PostgreSQL) 16.4").unwrap(),
        "16"
    );
}

#[test]
fn dump_target_applies_default_ports_and_decodes_components() {
    let target = DumpTarget::parse("postgres://user%2Bname:pa%25ss@db.example.com/app%2Ddb")
        .expect("target should parse");

    assert_eq!(target.scheme, "postgres");
    assert_eq!(target.host, "db.example.com");
    assert_eq!(target.port, 5432);
    assert_eq!(target.username, "user+name");
    assert_eq!(target.password.as_deref(), Some("pa%ss"));
    assert_eq!(target.database, "app-db");

    let target = DumpTarget::parse("mysql://user:pass@db.example.com:3307/app")
        .expect("target should parse");
    assert_eq!(target.port, 3307);
}

#[test]
fn dump_target_rejects_missing_username_database_and_unsupported_scheme() {
    assert!(matches!(
        DumpTarget::parse("postgres://localhost/app"),
        Err(AppError::Validation(message)) if message == "db_url username is required"
    ));
    assert!(matches!(
        DumpTarget::parse("postgres://user:pass@localhost"),
        Err(AppError::Validation(message)) if message == "db_url database name is required"
    ));
    assert!(matches!(
        DumpTarget::parse("sqlite://user:pass@localhost/app"),
        Err(AppError::Validation(message)) if message == "unsupported db_url scheme"
    ));
}

#[test]
fn parses_pg_dump_major_version_from_distribution_output() {
    assert_eq!(
        postgres_major_from_pg_dump_version("pg_dump (Ubuntu 16.9-0ubuntu0.24.04.1) 16.9").unwrap(),
        "16"
    );
    assert!(postgres_major_from_pg_dump_version("pg_dump version unknown").is_err());
}

#[test]
fn mysql_option_file_contains_credentials_without_url_arguments() {
    let target = DumpTarget::parse("mysql://user:secret@localhost/app").unwrap();
    let option_file = mysql_option_file(&target).unwrap();
    let contents = std::fs::read_to_string(option_file.path()).unwrap();

    assert!(contents.contains("[client]"));
    assert!(contents.contains("host=\"localhost\""));
    assert!(contents.contains("port=3306"));
    assert!(contents.contains("user=\"user\""));
    assert!(contents.contains("password=\"secret\""));
}

#[test]
fn mysql_option_file_quotes_backslashes_and_quotes() {
    let target = DumpTarget::parse("mysql://user:p%5C%22word@localhost/app").unwrap();
    let option_file = mysql_option_file(&target).unwrap();
    let contents = std::fs::read_to_string(option_file.path()).unwrap();

    assert!(contents.contains("password=\"p\\\\\\\"word\""));
}

#[test]
fn partial_output_reservation_is_unique_and_does_not_truncate() {
    use std::io::Write as _;

    let directory = tempfile::tempdir().unwrap();
    let history_id = Uuid::new_v4();
    let (first_path, mut first_file) =
        reserve_partial_output(directory.path(), history_id).unwrap();
    first_file.write_all(b"original").unwrap();
    drop(first_file);

    let collision = open_new_output_file(&first_path).unwrap_err();
    assert_eq!(collision.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&first_path).unwrap(), b"original");

    let (second_path, second_file) = reserve_partial_output(directory.path(), history_id).unwrap();
    drop(second_file);
    assert_ne!(first_path, second_path);
    assert!(first_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .ends_with(".part"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&first_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[tokio::test]
async fn finalizing_partial_output_syncs_and_renames_it() {
    use std::io::Write as _;

    let directory = tempfile::tempdir().unwrap();
    let (partial_path, mut partial_file) =
        reserve_partial_output(directory.path(), Uuid::new_v4()).unwrap();
    partial_file.write_all(b"backup contents").unwrap();
    drop(partial_file);
    let output_path = directory.path().join("backup.sql");

    finalize_partial_output(&partial_path, &output_path)
        .await
        .unwrap();

    assert!(!partial_path.exists());
    assert_eq!(std::fs::read(output_path).unwrap(), b"backup contents");
}

#[tokio::test]
async fn partial_sweep_preserves_non_partial_files() {
    let directory = tempfile::tempdir().unwrap();
    let partial_path = directory.path().join("abandoned.part");
    let backup_path = directory.path().join("keep.sql");
    std::fs::write(&partial_path, b"partial").unwrap();
    std::fs::write(&backup_path, b"backup").unwrap();

    sweep_partial_outputs(directory.path()).await;

    assert!(!partial_path.exists());
    assert!(backup_path.exists());
}

#[test]
fn final_file_name_validation_rejects_path_traversal() {
    assert!(is_safe_final_file_name("backup.sql"));
    assert!(!is_safe_final_file_name("../backup.sql"));
    assert!(!is_safe_final_file_name("nested/backup.sql"));
    assert!(!is_safe_final_file_name("backup.sql.part"));
}

#[tokio::test]
async fn startup_recovery_preserves_a_durable_final_and_removes_its_partial() {
    let backup_directory = tempfile::tempdir().unwrap();
    let final_path = backup_directory.path().join("completed.sql");
    let partial_path = backup_directory.path().join("abandoned.part");
    std::fs::write(&final_path, b"durable backup").unwrap();
    std::fs::write(&partial_path, b"partial").unwrap();
    let backup = InterruptedBackup {
        id: Uuid::new_v4(),
        file_name: Some("completed.sql".into()),
        file_path: Some(partial_path.to_string_lossy().to_string()),
    };

    assert_eq!(
        find_durable_final_output(backup_directory.path(), &backup)
            .await
            .unwrap(),
        Some(DurableFinalOutput {
            path: final_path.clone(),
            size: 14,
        })
    );
    cleanup_interrupted_partial(backup_directory.path(), &backup)
        .await
        .unwrap();

    assert_eq!(std::fs::read(final_path).unwrap(), b"durable backup");
    assert!(!partial_path.exists());
}

#[tokio::test]
async fn startup_recovery_rejects_a_missing_final_and_cleans_its_partial() {
    let backup_directory = tempfile::tempdir().unwrap();
    let partial_path = backup_directory.path().join("abandoned.part");
    std::fs::write(&partial_path, b"partial").unwrap();
    let backup = InterruptedBackup {
        id: Uuid::new_v4(),
        file_name: Some("missing.sql".into()),
        file_path: Some(partial_path.to_string_lossy().to_string()),
    };

    assert_eq!(
        find_durable_final_output(backup_directory.path(), &backup)
            .await
            .unwrap(),
        None
    );
    cleanup_interrupted_partial(backup_directory.path(), &backup)
        .await
        .unwrap();

    assert!(!partial_path.exists());
}

#[tokio::test]
async fn startup_recovery_rejects_a_directory_and_an_unsafe_final_name() {
    let root = tempfile::tempdir().unwrap();
    let backup_directory = root.path().join("backups");
    std::fs::create_dir(&backup_directory).unwrap();
    std::fs::create_dir(backup_directory.join("directory.sql")).unwrap();
    let outside_path = root.path().join("outside.sql");
    std::fs::write(&outside_path, b"outside").unwrap();

    let directory = InterruptedBackup {
        id: Uuid::new_v4(),
        file_name: Some("directory.sql".into()),
        file_path: None,
    };
    let unsafe_name = InterruptedBackup {
        id: Uuid::new_v4(),
        file_name: Some("../outside.sql".into()),
        file_path: None,
    };

    assert_eq!(
        find_durable_final_output(&backup_directory, &directory)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        find_durable_final_output(&backup_directory, &unsafe_name)
            .await
            .unwrap(),
        None
    );
    assert!(backup_directory.join("directory.sql").is_dir());
    assert_eq!(std::fs::read(outside_path).unwrap(), b"outside");
}

#[cfg(unix)]
#[tokio::test]
async fn startup_recovery_never_follows_a_final_symlink() {
    use std::os::unix::fs::symlink;

    let backup_directory = tempfile::tempdir().unwrap();
    let outside_directory = tempfile::tempdir().unwrap();
    let outside_path = outside_directory.path().join("outside.sql");
    let link_path = backup_directory.path().join("completed.sql");
    std::fs::write(&outside_path, b"outside").unwrap();
    symlink(&outside_path, &link_path).unwrap();
    let backup = InterruptedBackup {
        id: Uuid::new_v4(),
        file_name: Some("completed.sql".into()),
        file_path: None,
    };

    assert_eq!(
        find_durable_final_output(backup_directory.path(), &backup)
            .await
            .unwrap(),
        None
    );
    assert!(std::fs::symlink_metadata(link_path)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read(outside_path).unwrap(), b"outside");
}

#[tokio::test]
async fn managed_file_removal_handles_missing_and_outside_paths_safely() {
    let backup_directory = tempfile::tempdir().unwrap();
    let outside_directory = tempfile::tempdir().unwrap();
    let missing = backup_directory.path().join("missing.sql");
    assert_eq!(
        remove_managed_file(backup_directory.path(), &missing)
            .await
            .unwrap(),
        ManagedFileRemoval::Missing
    );

    let outside = outside_directory.path().join("outside.sql");
    std::fs::write(&outside, b"do not remove").unwrap();
    assert_eq!(
        remove_managed_file(backup_directory.path(), &outside)
            .await
            .unwrap(),
        ManagedFileRemoval::OutsideBackupDir
    );
    assert!(outside.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn managed_file_removal_unlinks_a_symlink_without_following_it() {
    use std::os::unix::fs::symlink;

    let backup_directory = tempfile::tempdir().unwrap();
    let outside_directory = tempfile::tempdir().unwrap();
    let outside = outside_directory.path().join("outside.sql");
    let link = backup_directory.path().join("managed.sql");
    std::fs::write(&outside, b"preserve me").unwrap();
    symlink(&outside, &link).unwrap();

    assert_eq!(
        remove_managed_file(backup_directory.path(), &link)
            .await
            .unwrap(),
        ManagedFileRemoval::Removed
    );
    assert!(!link.exists());
    assert_eq!(std::fs::read(outside).unwrap(), b"preserve me");
}

#[cfg(unix)]
#[tokio::test]
async fn command_timeout_terminates_descendants_without_waiting_on_their_pipes() {
    let mut command = Command::new("sh");
    command.arg("-c").arg("sleep 30 & wait");
    let started = std::time::Instant::now();

    assert!(matches!(
        run_command_output("sh", command, 1).await,
        Err(AppError::BackupTimeout)
    ));
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to a disposable PostgreSQL database"]
async fn periodic_retention_sweeps_disabled_configs_without_a_new_success() {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point to a disposable PostgreSQL database");
    let pool = crate::db::connect(&database_url).await.unwrap();
    crate::db::migrate(&pool).await.unwrap();
    let backup_directory = tempfile::tempdir().unwrap();
    let config_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO backup_configs
            (id, name, db_type, db_url_encrypted, db_url_nonce, is_enabled, retention_days)
        VALUES ($1, $2, 'postgres', 'unused', 'unused', false, 1)
        "#,
    )
    .bind(config_id)
    .bind(format!("periodic-retention-{config_id}"))
    .execute(&pool)
    .await
    .unwrap();

    let history_id = Uuid::new_v4();
    let backup_path = backup_directory.path().join(format!("{history_id}.sql"));
    std::fs::write(&backup_path, b"expired backup").unwrap();
    sqlx::query(
        r#"
        INSERT INTO backup_history
            (id, config_id, status, file_name, file_size, file_path,
             started_at, completed_at, triggered_by)
        VALUES ($1, $2, 'success', $3, 14, $4,
                now() - interval '2 days', now() - interval '2 days', 'scheduled')
        "#,
    )
    .bind(history_id)
    .bind(config_id)
    .bind(format!("{history_id}.sql"))
    .bind(backup_path.to_string_lossy().to_string())
    .execute(&pool)
    .await
    .unwrap();

    let runtime = BackupRuntime {
        db: pool.clone(),
        config: Arc::new(AppConfig {
            database_url,
            jwt_secret: "test-jwt-secret".into(),
            database_encryption_key: "test-encryption-key".into(),
            previous_database_encryption_key: None,
            backup_dir: backup_directory.path().to_path_buf(),
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            admin_username: "admin".into(),
            admin_password: "test-password".into(),
            reset_admin_password_on_start: false,
            jwt_ttl_seconds: 60,
            max_concurrent_backups: 1,
            cors_allowed_origin: None,
            smtp: None,
        }),
        permits: Arc::new(Semaphore::new(1)),
    };

    // A concurrent policy relaxation must win before retention reads its rules. Holding the
    // config row lock makes the retention task wait; after commit it must observe 365 days and
    // preserve this two-day-old file.
    let mut policy_transaction = pool.begin().await.unwrap();
    sqlx::query("UPDATE backup_configs SET retention_days = 365 WHERE id = $1")
        .bind(config_id)
        .execute(&mut *policy_transaction)
        .await
        .unwrap();
    let retention_runtime = runtime.clone();
    let mut retention_task =
        tokio::spawn(async move { apply_retention(&retention_runtime, config_id).await });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut retention_task)
            .await
            .is_err(),
        "retention should serialize with a policy update"
    );
    policy_transaction.commit().await.unwrap();
    retention_task.await.unwrap().unwrap();
    let preserved_path: Option<String> =
        sqlx::query_scalar("SELECT file_path FROM backup_history WHERE id = $1")
            .bind(history_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        preserved_path.as_deref(),
        Some(backup_path.to_string_lossy().as_ref())
    );
    let pending_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pending_file_deletions WHERE file_path = $1")
            .bind(backup_path.to_string_lossy().to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(pending_count, 0);

    sqlx::query("UPDATE backup_configs SET retention_days = 1 WHERE id = $1")
        .bind(config_id)
        .execute(&pool)
        .await
        .unwrap();
    let (_, failures) = sweep_all_retention(&runtime).await.unwrap();
    assert_eq!(failures, 0);
    assert!(backup_path.exists());
    let file_path: Option<String> =
        sqlx::query_scalar("SELECT file_path FROM backup_history WHERE id = $1")
            .bind(history_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(file_path.is_none());
    let pending_path: String =
        sqlx::query_scalar("SELECT file_path FROM pending_file_deletions WHERE file_path = $1")
            .bind(backup_path.to_string_lossy().to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(pending_path, backup_path.to_string_lossy());

    let purge = crate::services::file_deletion::purge_pending_file_deletions(
        &pool,
        backup_directory.path(),
    )
    .await
    .unwrap();
    assert_eq!(purge.deleted, 1);
    assert!(!backup_path.exists());

    sqlx::query("DELETE FROM backup_configs WHERE id = $1")
        .bind(config_id)
        .execute(&pool)
        .await
        .unwrap();
}
