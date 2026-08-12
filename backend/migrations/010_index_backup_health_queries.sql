CREATE INDEX backup_history_success_config_completed_idx
    ON backup_history (config_id, completed_at DESC)
    WHERE status = 'success';
