CREATE TABLE backup_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    history_id UUID NOT NULL REFERENCES backup_history(id) ON DELETE CASCADE,
    sequence BIGSERIAL NOT NULL,
    stage VARCHAR(32) NOT NULL,
    level VARCHAR(16) NOT NULL,
    message TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT backup_events_level_check CHECK (level IN ('info', 'success', 'warning', 'error'))
);

CREATE INDEX backup_events_history_sequence_idx ON backup_events (history_id, sequence);
CREATE INDEX backup_events_created_idx ON backup_events (created_at DESC);
