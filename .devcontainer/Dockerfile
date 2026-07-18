# Dev container image for DumplySquirrel.
#
# Based on the official Rust dev container image (ships a non-root `vscode`
# user, rustup, cargo and passwordless sudo). Node.js is layered on via a
# dev container feature in devcontainer.json. Here we add the system libraries
# needed to build the backend plus the database client tools that backup jobs
# shell out to.
FROM mcr.microsoft.com/devcontainers/rust:1-bookworm

# Build dependencies (OpenSSL/pkg-config for the sqlx + lettre stack) and the
# database clients used by backup jobs: mysql plus every supported Postgres
# major (14-18) from the PGDG apt repo, mirroring backend/Dockerfile.dev.
RUN apt-get update && export DEBIAN_FRONTEND=noninteractive \
    && apt-get install -y --no-install-recommends \
        pkg-config \
        libssl-dev \
        ca-certificates \
        wget \
        gnupg \
        default-mysql-client \
    && . /etc/os-release \
    && wget --quiet -O /usr/share/keyrings/postgresql.asc https://www.postgresql.org/media/keys/ACCC4CF8.asc \
    && echo "deb [signed-by=/usr/share/keyrings/postgresql.asc] https://apt.postgresql.org/pub/repos/apt ${VERSION_CODENAME}-pgdg main" > /etc/apt/sources.list.d/pgdg.list \
    && apt-get update && apt-get install -y --no-install-recommends \
        postgresql-client-14 \
        postgresql-client-15 \
        postgresql-client-16 \
        postgresql-client-17 \
        postgresql-client-18 \
    && apt-get clean && rm -rf /var/lib/apt/lists/*

# Ensure the common Rust dev components are present (idempotent).
RUN rustup component add rustfmt clippy

RUN curl -fsSL https://chatgpt.com/codex/install.sh | sh

RUN curl -fsSL https://claude.ai/install.sh | bash
