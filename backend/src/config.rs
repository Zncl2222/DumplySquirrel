use std::{env, net::SocketAddr, path::PathBuf};

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub database_url: String,
    pub jwt_secret: String,
    pub database_encryption_key: String,
    pub backup_dir: PathBuf,
    pub bind_addr: SocketAddr,
    pub admin_username: String,
    pub admin_password: String,
    pub jwt_ttl_seconds: i64,
    pub max_concurrent_backups: usize,
    pub cors_allowed_origin: Option<String>,
}

impl AppConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let database_url = required_env("DATABASE_URL")?;
        let jwt_secret = required_env("JWT_SECRET")?;
        let database_encryption_key = required_env("DATABASE_ENCRYPTION_KEY")?;
        let backup_dir =
            PathBuf::from(env::var("BACKUP_DIR").unwrap_or_else(|_| "./backups".into()));
        let bind_addr = env::var("BIND_ADDR")
            .unwrap_or_else(|_| "0.0.0.0:3000".into())
            .parse()?;
        let admin_username = env::var("ADMIN_USERNAME").unwrap_or_else(|_| "admin".into());
        let admin_password = required_env("ADMIN_PASSWORD")?;
        let jwt_ttl_seconds = env::var("JWT_TTL_SECONDS")
            .unwrap_or_else(|_| "28800".into())
            .parse()?;
        let max_concurrent_backups = env::var("MAX_CONCURRENT_BACKUPS")
            .unwrap_or_else(|_| "2".into())
            .parse()?;
        let cors_allowed_origin = env::var("CORS_ALLOWED_ORIGIN")
            .ok()
            .filter(|value| !value.trim().is_empty());

        if jwt_ttl_seconds < 60 {
            anyhow::bail!("JWT_TTL_SECONDS must be at least 60");
        }
        if max_concurrent_backups == 0 {
            anyhow::bail!("MAX_CONCURRENT_BACKUPS must be greater than 0");
        }

        Ok(Self {
            database_url,
            jwt_secret,
            database_encryption_key,
            backup_dir,
            bind_addr,
            admin_username,
            admin_password,
            jwt_ttl_seconds,
            max_concurrent_backups,
            cors_allowed_origin,
        })
    }
}

fn required_env(name: &str) -> anyhow::Result<String> {
    env::var(name).map_err(|_| anyhow::anyhow!("missing required environment variable {name}"))
}
