#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENV_FILE="${ENV_FILE:-$ROOT_DIR/.env}"

if [ -f "$ENV_FILE" ]; then
  set -a
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  set +a
fi

: "${DB_PASSWORD:?missing DB_PASSWORD in .env}"
: "${JWT_SECRET:?missing JWT_SECRET in .env}"
: "${DATABASE_ENCRYPTION_KEY:?missing DATABASE_ENCRYPTION_KEY in .env}"
: "${ADMIN_PASSWORD:?missing ADMIN_PASSWORD in .env}"

DEV_DB_NAME="${DEV_DB_NAME:-dumply_squirrel_dev}"
DEV_POSTGRES_PORT="${DEV_POSTGRES_PORT:-5432}"
DEV_BACKEND_PORT="${DEV_BACKEND_PORT:-8000}"
DEV_BACKUP_DIR="${DEV_BACKUP_DIR:-$ROOT_DIR/backups-dev}"

mkdir -p "$DEV_BACKUP_DIR"

if [ "${SKIP_DEV_DB_UP:-false}" != "true" ]; then
  "$ROOT_DIR/scripts/dev-db-up.sh"
fi

docker compose -f "$ROOT_DIR/docker-compose.dev.yml" stop backend >/dev/null 2>&1 || true

export DATABASE_URL="postgres://dumply:${DB_PASSWORD}@127.0.0.1:${DEV_POSTGRES_PORT}/${DEV_DB_NAME}"
export BIND_ADDR="${DEV_BACKEND_BIND_ADDR:-127.0.0.1:${DEV_BACKEND_PORT}}"
export BACKUP_DIR="$DEV_BACKUP_DIR"
export RUST_LOG="${DEV_RUST_LOG:-debug}"
export CORS_ALLOWED_ORIGIN="${DEV_CORS_ALLOWED_ORIGIN:-}"

exec cargo run --manifest-path "$ROOT_DIR/backend/Cargo.toml" "$@"
