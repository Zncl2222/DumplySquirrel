CREATE TABLE backup_history (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    config_id UUID NOT NULL REFERENCES backup_configs(id) ON DELETE CASCADE,
    status VARCHAR(20) NOT NULL,
    file_name VARCHAR(255),
    file_size BIGINT,
    file_path TEXT,
    error_message TEXT,
    started_at TIMESTAMPTZ NOT NULL,
    completed_at TIMESTAMPTZ,
    triggered_by VARCHAR(20) NOT NULL,
    CONSTRAINT backup_history_status_check CHECK (status IN ('running', 'success', 'failed', 'timeout', 'cancelled')),
    CONSTRAINT backup_history_triggered_by_check CHECK (triggered_by IN ('manual', 'scheduled'))
);

CREATE INDEX backup_history_config_started_idx ON backup_history (config_id, started_at DESC);
CREATE INDEX backup_history_status_idx ON backup_history (status);
CREATE UNIQUE INDEX backup_history_one_running_per_config_idx
    ON backup_history (config_id)
    WHERE status = 'running';
