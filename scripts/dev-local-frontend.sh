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

PACKAGE_MANAGER="${1:-npm}"
case "$PACKAGE_MANAGER" in
  npm|bun)
    shift || true
    ;;
  *)
    echo "Usage: $0 [npm|bun] [extra vite args...]" >&2
    exit 1
    ;;
esac

DEV_FRONTEND_PORT="${DEV_FRONTEND_PORT:-5173}"
export VITE_API_BASE_URL="${DEV_FRONTEND_API_BASE_URL:-/api}"
export VITE_DEV_PROXY_TARGET="${DEV_LOCAL_FRONTEND_PROXY_TARGET:-http://127.0.0.1:${DEV_BACKEND_PORT:-8000}}"

docker compose -f "$ROOT_DIR/docker-compose.dev.yml" stop frontend >/dev/null 2>&1 || true

cd "$ROOT_DIR/frontend"

if [ "$PACKAGE_MANAGER" = "bun" ]; then
  command -v bun >/dev/null 2>&1 || { echo "bun is not installed" >&2; exit 1; }
  if [ ! -d node_modules/.bin ]; then
    bun install
  fi
  exec bun run dev -- --host 127.0.0.1 --port "$DEV_FRONTEND_PORT" "$@"
fi

command -v npm >/dev/null 2>&1 || { echo "npm is not installed" >&2; exit 1; }
if [ ! -d node_modules/.bin ]; then
  npm ci
fi
exec npm run dev -- --host 127.0.0.1 --port "$DEV_FRONTEND_PORT" "$@"
