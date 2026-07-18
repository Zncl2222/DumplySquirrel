use super::*;

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to a disposable PostgreSQL database"]
async fn expired_instance_lease_holds_advisory_lock_until_shutdown() {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point to a disposable PostgreSQL database");
    let pool = connect(&database_url).await.unwrap();
    migrate(&pool).await.unwrap();
    sqlx::query("DELETE FROM runtime_instance_leases WHERE name = $1")
        .bind(INSTANCE_LEASE_NAME)
        .execute(&pool)
        .await
        .unwrap();

    let first = acquire_instance_lock(&pool).await.unwrap();
    let mut loss_receiver = first.loss_receiver();
    sqlx::query(
        "UPDATE runtime_instance_leases SET expires_at = now() - interval '1 second' WHERE name = $1",
    )
    .bind(INSTANCE_LEASE_NAME)
    .execute(&pool)
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !*loss_receiver.borrow() {
            loss_receiver.changed().await.unwrap();
        }
    })
    .await
    .expect("lease monitor should detect an expired ownership row");

    assert!(
        acquire_instance_lock(&pool).await.is_err(),
        "a live failed owner must retain its advisory lock until shutdown"
    );
    first.release().await;
    let replacement = acquire_instance_lock(&pool).await.unwrap();
    replacement.release().await;
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to a disposable PostgreSQL database"]
async fn instance_lease_blocks_takeover_after_advisory_session_loss() {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point to a disposable PostgreSQL database");
    let pool = connect(&database_url).await.unwrap();
    migrate(&pool).await.unwrap();
    sqlx::query("DELETE FROM runtime_instance_leases WHERE name = $1")
        .bind(INSTANCE_LEASE_NAME)
        .execute(&pool)
        .await
        .unwrap();

    let first = acquire_instance_lock(&pool).await.unwrap();
    assert!(acquire_instance_lock(&pool).await.is_err());
    let mut loss_receiver = first.loss_receiver();
    let backend_pid: i32 =
        sqlx::query_scalar("SELECT backend_pid FROM runtime_instance_leases WHERE name = $1")
            .bind(INSTANCE_LEASE_NAME)
            .fetch_one(&pool)
            .await
            .unwrap();
    let terminated: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)")
        .bind(backend_pid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(terminated);

    tokio::time::timeout(Duration::from_secs(10), async {
        while !*loss_receiver.borrow() {
            loss_receiver.changed().await.unwrap();
        }
    })
    .await
    .expect("lease monitor should detect its terminated PostgreSQL session");

    assert!(
        acquire_instance_lock(&pool).await.is_err(),
        "durable lease must block takeover after the advisory-lock session is lost"
    );
    drop(first);

    sqlx::query("DELETE FROM runtime_instance_leases WHERE name = $1")
        .bind(INSTANCE_LEASE_NAME)
        .execute(&pool)
        .await
        .unwrap();
    let replacement = acquire_instance_lock(&pool).await.unwrap();
    replacement.release().await;
}
