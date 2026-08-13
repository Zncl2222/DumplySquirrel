pub mod models;

use bcrypt::{hash, verify, DEFAULT_COST};
use sqlx::{postgres::PgPoolOptions, PgPool};
use tokio::{
    sync::{oneshot, watch},
    task::JoinHandle,
    time::{self, Duration, MissedTickBehavior},
};
use uuid::Uuid;

use crate::config::AppConfig;

const INSTANCE_ADVISORY_LOCK_ID: i64 = 0x4475_6d70_6c79_5371;
const INSTANCE_LEASE_NAME: &str = "backend";
const INSTANCE_LEASE_SECONDS: i64 = 30;
const INSTANCE_LEASE_RENEW_INTERVAL: Duration = Duration::from_secs(3);
const INSTANCE_LEASE_QUERY_TIMEOUT: Duration = Duration::from_secs(5);

/// Keeps DumplySquirrel single-writer for its database and managed backup directory.
///
/// Recovery and retention intentionally operate on process-owned files, so allowing multiple
/// backend replicas without an ownership lease could make one replica clean another's live run.
pub struct InstanceLock {
    loss_receiver: watch::Receiver<bool>,
    stop_sender: Option<oneshot::Sender<()>>,
    monitor_task: Option<JoinHandle<()>>,
}

impl InstanceLock {
    pub fn loss_receiver(&self) -> watch::Receiver<bool> {
        self.loss_receiver.clone()
    }

    pub async fn release(mut self) {
        if let Some(stop_sender) = self.stop_sender.take() {
            let _ = stop_sender.send(());
        }
        if let Some(monitor_task) = self.monitor_task.take() {
            if let Err(err) = monitor_task.await {
                tracing::warn!(error = ?err, "instance lease monitor did not stop cleanly");
            }
        }
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        if let Some(stop_sender) = self.stop_sender.take() {
            let _ = stop_sender.send(());
        }
    }
}

pub async fn connect(database_url: &str) -> anyhow::Result<PgPool> {
    Ok(PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(5))
        .idle_timeout(Some(Duration::from_secs(10 * 60)))
        .max_lifetime(Some(Duration::from_secs(30 * 60)))
        .connect(database_url)
        .await?)
}

pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

pub async fn acquire_instance_lock(pool: &PgPool) -> anyhow::Result<InstanceLock> {
    let mut connection = pool.acquire().await?;
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(INSTANCE_ADVISORY_LOCK_ID)
        .fetch_one(&mut *connection)
        .await?;
    if !acquired {
        anyhow::bail!(
            "another DumplySquirrel backend is already using this configuration database; only one backend replica is supported"
        );
    }

    let owner_id = Uuid::new_v4();
    let claimed_owner: Option<Uuid> = sqlx::query_scalar(
        r#"
        INSERT INTO runtime_instance_leases
            (name, owner_id, backend_pid, expires_at, updated_at)
        VALUES
            ($1, $2, pg_backend_pid(), now() + ($3::bigint * interval '1 second'), now())
        ON CONFLICT (name) DO UPDATE
        SET owner_id = EXCLUDED.owner_id,
            backend_pid = EXCLUDED.backend_pid,
            expires_at = EXCLUDED.expires_at,
            updated_at = now()
        WHERE runtime_instance_leases.expires_at <= now()
        RETURNING owner_id
        "#,
    )
    .bind(INSTANCE_LEASE_NAME)
    .bind(owner_id)
    .bind(INSTANCE_LEASE_SECONDS)
    .fetch_optional(&mut *connection)
    .await?;

    if claimed_owner != Some(owner_id) {
        let _: bool = sqlx::query_scalar("SELECT pg_advisory_unlock($1)")
            .bind(INSTANCE_ADVISORY_LOCK_ID)
            .fetch_one(&mut *connection)
            .await?;
        anyhow::bail!(
            "a previous DumplySquirrel instance lease is still active; retry after at most {INSTANCE_LEASE_SECONDS} seconds"
        );
    }

    let (loss_sender, loss_receiver) = watch::channel(false);
    let (stop_sender, mut stop_receiver) = oneshot::channel();
    let monitor_task = tokio::spawn(async move {
        let mut interval = time::interval(INSTANCE_LEASE_RENEW_INTERVAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        interval.tick().await;

        let lease_lost = loop {
            tokio::select! {
                _ = interval.tick() => {
                    // A half-open connection must not leave this process writing shared files
                    // after its durable lease can be claimed by a replacement instance.
                    let renewed = time::timeout(
                        INSTANCE_LEASE_QUERY_TIMEOUT,
                        sqlx::query(
                            r#"
                            UPDATE runtime_instance_leases
                            SET expires_at = now() + ($3::bigint * interval '1 second'),
                                updated_at = now(),
                                backend_pid = pg_backend_pid()
                            WHERE name = $1 AND owner_id = $2 AND expires_at > now()
                            "#,
                        )
                        .bind(INSTANCE_LEASE_NAME)
                        .bind(owner_id)
                        .bind(INSTANCE_LEASE_SECONDS)
                        .execute(&mut *connection),
                    )
                    .await;

                    match renewed {
                        Ok(Ok(result)) if result.rows_affected() == 1 => {}
                        Ok(Ok(_)) => {
                            tracing::error!(%owner_id, "instance lease was lost or expired; shutting down");
                            let _ = loss_sender.send(true);
                            break true;
                        }
                        Ok(Err(err)) => {
                            tracing::error!(%owner_id, error = ?err, "instance lease heartbeat failed; shutting down");
                            let _ = loss_sender.send(true);
                            break true;
                        }
                        Err(_) => {
                            tracing::error!(%owner_id, timeout_seconds = INSTANCE_LEASE_QUERY_TIMEOUT.as_secs(), "instance lease heartbeat timed out; shutting down");
                            let _ = loss_sender.send(true);
                            break true;
                        }
                    }
                }
                _ = &mut stop_receiver => {
                    break false;
                }
            }
        };

        if lease_lost {
            // Keep a still-live session (and therefore its advisory lock) until the server has
            // actually stopped. The durable lease covers the case where the session itself died.
            let _ = stop_receiver.await;
        }
        if let Err(err) =
            sqlx::query("DELETE FROM runtime_instance_leases WHERE name = $1 AND owner_id = $2")
                .bind(INSTANCE_LEASE_NAME)
                .bind(owner_id)
                .execute(&mut *connection)
                .await
        {
            tracing::warn!(%owner_id, error = ?err, "failed to release instance lease row");
        }
        if let Err(err) = sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1)")
            .bind(INSTANCE_ADVISORY_LOCK_ID)
            .fetch_one(&mut *connection)
            .await
        {
            tracing::warn!(%owner_id, error = ?err, "failed to release instance advisory lock");
        }
    });

    Ok(InstanceLock {
        loss_receiver,
        stop_sender: Some(stop_sender),
        monitor_task: Some(monitor_task),
    })
}

#[cfg(test)]
mod tests;

pub async fn bootstrap_admin(pool: &PgPool, config: &AppConfig) -> anyhow::Result<()> {
    let user_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await?;

    if user_count > 0 {
        let existing = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, password_hash FROM users WHERE username = $1",
        )
        .bind(&config.admin_username)
        .fetch_optional(pool)
        .await?;
        let uses_public_default = if let Some((_, password_hash)) = &existing {
            let password_hash = password_hash.clone();
            tokio::task::spawn_blocking(move || verify("change_me_in_production", &password_hash))
                .await??
        } else {
            false
        };
        let should_reset = config.reset_admin_password_on_start
            || (uses_public_default && config.admin_password != "change_me_in_production");
        if should_reset {
            let Some((user_id, _)) = existing else {
                anyhow::bail!(
                    "RESET_ADMIN_PASSWORD_ON_START is true, but ADMIN_USERNAME does not match an existing user"
                );
            };
            let password = config.admin_password.clone();
            let password_hash =
                tokio::task::spawn_blocking(move || hash(password, DEFAULT_COST)).await??;
            sqlx::query(
                r#"
                UPDATE users
                SET password_hash = $2, token_version = token_version + 1, updated_at = now()
                WHERE id = $1
                "#,
            )
            .bind(user_id)
            .bind(password_hash)
            .execute(pool)
            .await?;
            tracing::warn!(
                username = %config.admin_username,
                automatic_public_default_rotation = uses_public_default,
                "admin password reset from startup configuration; set RESET_ADMIN_PASSWORD_ON_START=false after this start"
            );
        }
        return Ok(());
    }

    let password = config.admin_password.clone();
    let password_hash = tokio::task::spawn_blocking(move || hash(password, DEFAULT_COST)).await??;
    let result = sqlx::query(
        r#"
        INSERT INTO users (id, username, password_hash, role)
        VALUES ($1, $2, $3, 'admin')
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(&config.admin_username)
    .bind(password_hash)
    .execute(pool)
    .await;

    if let Err(sqlx::Error::Database(err)) = &result {
        if err.code().as_deref() == Some("23505") {
            tracing::info!(username = %config.admin_username, "bootstrap admin already created by another instance");
            return Ok(());
        }
    }
    result?;

    tracing::info!(username = %config.admin_username, "bootstrap admin user created");
    Ok(())
}
