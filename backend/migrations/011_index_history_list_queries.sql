CREATE INDEX backup_history_started_id_idx
    ON backup_history (started_at DESC, id DESC);

CREATE INDEX backup_history_status_started_id_idx
    ON backup_history (status, started_at DESC, id DESC);

CREATE INDEX backup_history_config_status_started_id_idx
    ON backup_history (config_id, status, started_at DESC, id DESC);

-- The composite status index serves the same equality lookups while also satisfying list order.
DROP INDEX backup_history_status_idx;
