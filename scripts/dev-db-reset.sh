#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

docker compose -f "$ROOT_DIR/docker-compose.dev.yml" rm -sf postgres || true
docker volume rm dumplysquirrel_pgdata_dev || true
