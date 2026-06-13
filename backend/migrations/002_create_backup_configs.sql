CREATE TABLE backup_configs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(200) NOT NULL,
    db_type VARCHAR(20) NOT NULL,
    db_url_encrypted TEXT NOT NULL,
    db_url_nonce TEXT NOT NULL,
    cron_schedule VARCHAR(100),
    is_enabled BOOLEAN NOT NULL DEFAULT true,
    retention_days INTEGER NOT NULL DEFAULT 30,
    timeout_seconds INTEGER NOT NULL DEFAULT 3600,
    max_backups INTEGER,
    created_by UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT backup_configs_db_type_check CHECK (db_type IN ('postgres', 'mysql')),
    CONSTRAINT backup_configs_retention_days_check CHECK (retention_days > 0),
    CONSTRAINT backup_configs_timeout_seconds_check CHECK (timeout_seconds > 0),
    CONSTRAINT backup_configs_max_backups_check CHECK (max_backups IS NULL OR max_backups > 0)
);

CREATE INDEX backup_configs_enabled_idx ON backup_configs (is_enabled);
