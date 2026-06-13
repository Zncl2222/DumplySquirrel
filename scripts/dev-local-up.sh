#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PACKAGE_MANAGER="${1:-npm}"

"$ROOT_DIR/scripts/dev-db-up.sh"

cleanup() {
  kill "$BACKEND_PID" "$FRONTEND_PID" >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

SKIP_DEV_DB_UP=true "$ROOT_DIR/scripts/dev-local-backend.sh" &
BACKEND_PID=$!

sleep 2

"$ROOT_DIR/scripts/dev-local-frontend.sh" "$PACKAGE_MANAGER" &
FRONTEND_PID=$!

wait -n "$BACKEND_PID" "$FRONTEND_PID"
