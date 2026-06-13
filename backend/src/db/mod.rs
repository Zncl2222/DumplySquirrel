pub mod models;

use bcrypt::{hash, DEFAULT_COST};
use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

use crate::config::AppConfig;

pub async fn connect(database_url: &str) -> anyhow::Result<PgPool> {
    Ok(PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await?)
}

pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

pub async fn bootstrap_admin(pool: &PgPool, config: &AppConfig) -> anyhow::Result<()> {
    let user_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await?;

    if user_count > 0 {
        return Ok(());
    }

    let password_hash = hash(&config.admin_password, DEFAULT_COST)?;
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
