use std::{env, net::SocketAddr, path::PathBuf};

use percent_encoding::percent_decode_str;

const MAX_SAFE_CONCURRENT_BACKUPS: usize = 32;
const MIN_SAFE_BACKUP_FILE_BYTES: u64 = 1024 * 1024;
const MIN_SAFE_FREE_DISK_BYTES: u64 = 64 * 1024 * 1024;
const DEFAULT_MAX_BACKUP_FILE_BYTES: u64 = 100 * 1024 * 1024 * 1024;
const DEFAULT_MIN_FREE_DISK_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub database_url: String,
    pub jwt_secret: String,
    pub database_encryption_key: String,
    pub previous_database_encryption_key: Option<String>,
    pub backup_dir: PathBuf,
    pub bind_addr: SocketAddr,
    pub admin_username: String,
    pub admin_password: String,
    pub reset_admin_password_on_start: bool,
    pub jwt_ttl_seconds: i64,
    pub max_concurrent_backups: usize,
    pub max_backup_file_bytes: u64,
    pub min_free_disk_bytes: u64,
    pub cors_allowed_origin: Option<String>,
    pub smtp: Option<SmtpConfig>,
}

#[derive(Debug, Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from: String,
    pub tls: SmtpTlsMode,
}

#[derive(Debug, Clone, Copy)]
pub enum SmtpTlsMode {
    StartTls,
    None,
}

impl AppConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let allow_insecure_dev_secrets = bool_env("ALLOW_INSECURE_DEV_SECRETS", false)?;
        let database_url = required_env("DATABASE_URL")?;
        let jwt_secret = required_env("JWT_SECRET")?;
        let database_encryption_key = required_env("DATABASE_ENCRYPTION_KEY")?;
        let previous_database_encryption_key = optional_env("DATABASE_ENCRYPTION_KEY_PREVIOUS");
        let backup_dir =
            PathBuf::from(env::var("BACKUP_DIR").unwrap_or_else(|_| "./backups".into()));
        let bind_addr = env::var("BIND_ADDR")
            .unwrap_or_else(|_| "0.0.0.0:3000".into())
            .parse()?;
        let admin_username = validate_username(
            "ADMIN_USERNAME",
            &env::var("ADMIN_USERNAME").unwrap_or_else(|_| "admin".into()),
        )?;
        let admin_password = required_env("ADMIN_PASSWORD")?;
        let reset_admin_password_on_start = bool_env("RESET_ADMIN_PASSWORD_ON_START", false)?;
        let jwt_ttl_seconds = env::var("JWT_TTL_SECONDS")
            .unwrap_or_else(|_| "28800".into())
            .parse()?;
        let max_concurrent_backups = env::var("MAX_CONCURRENT_BACKUPS")
            .unwrap_or_else(|_| "2".into())
            .parse()?;
        let max_backup_file_bytes = env::var("MAX_BACKUP_FILE_BYTES")
            .unwrap_or_else(|_| DEFAULT_MAX_BACKUP_FILE_BYTES.to_string())
            .parse()?;
        let min_free_disk_bytes = env::var("MIN_FREE_DISK_BYTES")
            .unwrap_or_else(|_| DEFAULT_MIN_FREE_DISK_BYTES.to_string())
            .parse()?;
        let cors_allowed_origin = env::var("CORS_ALLOWED_ORIGIN")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let smtp = smtp_config_from_env()?;

        validate_database_url_secret(&database_url, allow_insecure_dev_secrets)?;
        validate_secret("JWT_SECRET", &jwt_secret, 32, allow_insecure_dev_secrets)?;
        validate_secret(
            "DATABASE_ENCRYPTION_KEY",
            &database_encryption_key,
            32,
            allow_insecure_dev_secrets,
        )?;
        validate_secret(
            "ADMIN_PASSWORD",
            &admin_password,
            12,
            allow_insecure_dev_secrets,
        )?;
        if admin_password.len() > 72 {
            anyhow::bail!(
                "ADMIN_PASSWORD must be at most 72 bytes because bcrypt truncates longer values"
            );
        }

        if jwt_ttl_seconds < 60 {
            anyhow::bail!("JWT_TTL_SECONDS must be at least 60");
        }
        validate_max_concurrent_backups(max_concurrent_backups)?;
        validate_backup_storage_limits(max_backup_file_bytes, min_free_disk_bytes)?;

        Ok(Self {
            database_url,
            jwt_secret,
            database_encryption_key,
            previous_database_encryption_key,
            backup_dir,
            bind_addr,
            admin_username,
            admin_password,
            reset_admin_password_on_start,
            jwt_ttl_seconds,
            max_concurrent_backups,
            max_backup_file_bytes,
            min_free_disk_bytes,
            cors_allowed_origin,
            smtp,
        })
    }
}

fn smtp_config_from_env() -> anyhow::Result<Option<SmtpConfig>> {
    let host = optional_env("SMTP_HOST");
    let from = optional_env("SMTP_FROM");
    let (Some(host), Some(from)) = (host, from) else {
        return Ok(None);
    };

    let port = env::var("SMTP_PORT")
        .unwrap_or_else(|_| "587".into())
        .parse()?;
    let tls = match env::var("SMTP_TLS")
        .unwrap_or_else(|_| "starttls".into())
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "starttls" | "tls" => SmtpTlsMode::StartTls,
        "none" | "plain" => SmtpTlsMode::None,
        other => anyhow::bail!("SMTP_TLS must be starttls or none, got {other}"),
    };

    Ok(Some(SmtpConfig {
        host,
        port,
        username: optional_env("SMTP_USERNAME"),
        password: optional_env_raw("SMTP_PASSWORD"),
        from,
        tls,
    }))
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn optional_env_raw(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.is_empty())
}

fn required_env(name: &str) -> anyhow::Result<String> {
    let value = env::var(name)
        .map_err(|_| anyhow::anyhow!("missing required environment variable {name}"))?;
    let value = value.trim().to_string();
    if value.is_empty() {
        anyhow::bail!("environment variable {name} must not be empty");
    }
    Ok(value)
}

fn bool_env(name: &str, default: bool) -> anyhow::Result<bool> {
    let Some(value) = optional_env(name) else {
        return Ok(default);
    };
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => anyhow::bail!("environment variable {name} must be true or false"),
    }
}

fn validate_secret(
    name: &str,
    value: &str,
    minimum_bytes: usize,
    allow_insecure: bool,
) -> anyhow::Result<()> {
    if allow_insecure {
        return Ok(());
    }
    if is_known_placeholder(value) {
        anyhow::bail!(
            "{name} still uses a public placeholder; generate a unique secret before starting DumplySquirrel"
        );
    }
    if value.len() < minimum_bytes {
        anyhow::bail!("{name} must be at least {minimum_bytes} bytes");
    }
    Ok(())
}

fn validate_database_url_secret(database_url: &str, allow_insecure: bool) -> anyhow::Result<()> {
    if allow_insecure {
        return Ok(());
    }
    let parsed = url::Url::parse(database_url)
        .map_err(|_| anyhow::anyhow!("DATABASE_URL must be a valid URL"))?;
    let encoded_password = parsed
        .password()
        .ok_or_else(|| anyhow::anyhow!("DATABASE_URL must include a database password"))?;
    let password = percent_decode_str(encoded_password)
        .decode_utf8()
        .map_err(|_| anyhow::anyhow!("DATABASE_URL password must be valid UTF-8"))?;
    validate_secret("DATABASE_URL password", &password, 12, false)
}

fn validate_username(name: &str, value: &str) -> anyhow::Result<String> {
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("{name} must not be empty");
    }
    if value.len() > 100 {
        anyhow::bail!("{name} must be at most 100 bytes");
    }
    if value.chars().any(char::is_control) {
        anyhow::bail!("{name} must not contain control characters");
    }
    Ok(value.to_string())
}

fn validate_max_concurrent_backups(value: usize) -> anyhow::Result<()> {
    if value == 0 {
        anyhow::bail!("MAX_CONCURRENT_BACKUPS must be greater than 0");
    }
    if value > MAX_SAFE_CONCURRENT_BACKUPS {
        anyhow::bail!(
            "MAX_CONCURRENT_BACKUPS must not exceed {MAX_SAFE_CONCURRENT_BACKUPS}; run additional isolated workers instead of exhausting one host"
        );
    }
    Ok(())
}

fn validate_backup_storage_limits(max_file_bytes: u64, min_free_bytes: u64) -> anyhow::Result<()> {
    if max_file_bytes < MIN_SAFE_BACKUP_FILE_BYTES {
        anyhow::bail!("MAX_BACKUP_FILE_BYTES must be at least {MIN_SAFE_BACKUP_FILE_BYTES} bytes");
    }
    if max_file_bytes > i64::MAX as u64 {
        anyhow::bail!("MAX_BACKUP_FILE_BYTES must not exceed {} bytes", i64::MAX);
    }
    if min_free_bytes < MIN_SAFE_FREE_DISK_BYTES {
        anyhow::bail!("MIN_FREE_DISK_BYTES must be at least {MIN_SAFE_FREE_DISK_BYTES} bytes");
    }
    Ok(())
}

fn is_known_placeholder(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "change_me_in_production"
            | "change_me_in_production_32_bytes_minimum"
            | "change_me_32_bytes_minimum_secret"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_secrets_reject_public_placeholders_and_short_values() {
        assert!(validate_secret("JWT_SECRET", "change_me_in_production", 32, false).is_err());
        assert!(validate_secret("JWT_SECRET", "too-short", 32, false).is_err());
        assert!(validate_database_url_secret(
            "postgres://dumply:change_me_in_production@postgres/dumply",
            false,
        )
        .is_err());
        assert!(
            validate_database_url_secret("postgres://dumply:x@postgres/dumply", false).is_err()
        );
        assert!(validate_database_url_secret("postgres://dumply@postgres/dumply", false).is_err());
        assert!(validate_database_url_secret(
            "postgres://dumply:change%5Fme%5Fin%5Fproduction@postgres/dumply",
            false,
        )
        .is_err());
    }

    #[test]
    fn development_override_and_strong_secrets_are_accepted() {
        assert!(validate_secret("JWT_SECRET", "change_me_in_production", 32, true).is_ok());
        assert!(validate_secret(
            "JWT_SECRET",
            "a-unique-secret-with-at-least-thirty-two-bytes",
            32,
            false,
        )
        .is_ok());
        assert_eq!(
            validate_username("ADMIN_USERNAME", " admin ").unwrap(),
            "admin"
        );
        assert!(validate_max_concurrent_backups(1).is_ok());
        assert!(validate_max_concurrent_backups(MAX_SAFE_CONCURRENT_BACKUPS).is_ok());
        assert!(validate_max_concurrent_backups(0).is_err());
        assert!(validate_max_concurrent_backups(MAX_SAFE_CONCURRENT_BACKUPS + 1).is_err());
        assert!(validate_backup_storage_limits(
            DEFAULT_MAX_BACKUP_FILE_BYTES,
            DEFAULT_MIN_FREE_DISK_BYTES
        )
        .is_ok());
        assert!(validate_backup_storage_limits(MIN_SAFE_BACKUP_FILE_BYTES - 1, 64 << 20).is_err());
        assert!(validate_backup_storage_limits(u64::MAX, DEFAULT_MIN_FREE_DISK_BYTES).is_err());
        assert!(validate_backup_storage_limits(1 << 20, MIN_SAFE_FREE_DISK_BYTES - 1).is_err());
    }
}
