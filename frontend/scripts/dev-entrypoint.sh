#!/bin/sh
set -eu

if [ ! -d node_modules/.bin ]; then
  npm ci
fi

exec npm run dev -- --host 0.0.0.0 --port "${DEV_FRONTEND_PORT:-5173}"
