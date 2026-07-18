CREATE TABLE runtime_instance_leases (
    name TEXT PRIMARY KEY,
    owner_id UUID NOT NULL,
    backend_pid INTEGER NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX runtime_instance_leases_expires_idx
    ON runtime_instance_leases (expires_at);
