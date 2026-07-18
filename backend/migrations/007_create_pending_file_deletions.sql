CREATE TABLE pending_file_deletions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    file_path TEXT NOT NULL UNIQUE,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    last_attempt_at TIMESTAMPTZ,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT pending_file_deletions_path_not_blank CHECK (btrim(file_path) <> ''),
    CONSTRAINT pending_file_deletions_attempts_nonnegative CHECK (attempts >= 0)
);

CREATE INDEX pending_file_deletions_retry_idx
    ON pending_file_deletions (next_attempt_at, created_at, id);
