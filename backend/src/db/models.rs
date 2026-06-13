use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BackupConfig {
    pub id: Uuid,
    pub name: String,
    pub db_type: String,
    pub db_version: Option<String>,
    pub db_url_encrypted: String,
    pub db_url_nonce: String,
    pub cron_schedule: Option<String>,
    pub is_enabled: bool,
    pub retention_days: i32,
    pub timeout_seconds: i32,
    pub max_backups: Option<i32>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct BackupHistory {
    pub id: Uuid,
    pub config_id: Uuid,
    pub status: String,
    pub file_name: Option<String>,
    pub file_size: Option<i64>,
    #[serde(skip_serializing)]
    pub file_path: Option<String>,
    pub error_message: Option<String>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub triggered_by: String,
}
