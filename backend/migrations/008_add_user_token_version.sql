ALTER TABLE users
    ADD COLUMN token_version BIGINT NOT NULL DEFAULT 0;

ALTER TABLE users
    ADD CONSTRAINT users_token_version_nonnegative_check CHECK (token_version >= 0);
