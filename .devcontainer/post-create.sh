#!/usr/bin/env bash
# Runs once after the dev container is created. Warms the dependency caches so
# the first `cargo run` / `npm run dev` is fast.
set -euo pipefail

echo "==> Fetching Rust dependencies (backend)"
(cd backend && cargo fetch)

echo "==> Installing frontend dependencies"
(cd frontend && npm ci)

cat <<'EOF'

Dev container ready.

  Postgres      -> host "postgres:5432" (from this container) / localhost:5432
  Backend       -> cd backend && cargo run          (http://localhost:8000)
  Frontend      -> cd frontend && npm run dev        (http://localhost:5173)

DB migrations run automatically on backend startup.
EOF
