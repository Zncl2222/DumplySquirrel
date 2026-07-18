#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_DIR="$(mktemp -d)"
trap 'rm -rf -- "$TEST_DIR"' EXIT

ENV_FILE="$TEST_DIR/test.env"
COMPOSE_FILE="$TEST_DIR/compose.yml"
COMPOSE_JSON="$TEST_DIR/compose.json"

printf '%s\n' \
  'SMTP_HOST=' \
  'SMTP_PORT=' \
  'SMTP_USERNAME=' \
  'SMTP_PASSWORD=' \
  'SMTP_FROM=' \
  'SMTP_TLS=' > "$ENV_FILE"

expected_host='smtp.example.test'
expected_username='alerts#ops'
expected_password='pa$word ${HOME} # literal & pipe|value'
expected_from='Dumply # Alerts <dumply@example.test>'

{
  printf '%s\n' "$expected_host"
  printf '%s\n' '2525'
  printf '%s\n' "$expected_username"
  printf '%s\n' "$expected_password"
  printf '%s\n' "$expected_from"
  printf '%s\n' 'starttls'
} | sh "$ROOT_DIR/scripts/configure-email.sh" "$ENV_FILE" >/dev/null

# shellcheck disable=SC1091
source "$ROOT_DIR/scripts/load-env.sh" "$ENV_FILE"

[[ "$SMTP_HOST" == "$expected_host" ]]
[[ "$SMTP_PORT" == '2525' ]]
[[ "$SMTP_USERNAME" == "$expected_username" ]]
[[ "$SMTP_PASSWORD" == "$expected_password" ]]
[[ "$SMTP_FROM" == "$expected_from" ]]
[[ "$SMTP_TLS" == 'starttls' ]]
grep -Fq "SMTP_PASSWORD='" "$ENV_FILE"

for ambiguous_value in "can't-round-trip" 'back\slash'; do
  printf '%s\n' \
    'SMTP_HOST=old-host' \
    'SMTP_PORT=25' \
    'SMTP_USERNAME=old-user' \
    'SMTP_PASSWORD=old-password' \
    'SMTP_FROM=old-from' \
    'SMTP_TLS=none' > "$TEST_DIR/ambiguous.env"
  cp "$TEST_DIR/ambiguous.env" "$TEST_DIR/ambiguous.before"
  if printf '%s\n' '' '' '' "$ambiguous_value" '' '' | \
    sh "$ROOT_DIR/scripts/configure-email.sh" "$TEST_DIR/ambiguous.env" >/dev/null 2>&1; then
    printf 'ambiguous quote/backslash value was unexpectedly accepted\n' >&2
    exit 1
  fi
  cmp "$TEST_DIR/ambiguous.before" "$TEST_DIR/ambiguous.env"
done

# A pre-existing dotenv without a final newline must not merge its last key with an appended key.
printf '%s' 'KEEP=original' > "$TEST_DIR/no-final-newline.env"
printf '%s\n' 'host' '587' 'user' 'password' 'from@example.test' 'starttls' | \
  sh "$ROOT_DIR/scripts/configure-email.sh" "$TEST_DIR/no-final-newline.env" >/dev/null
# shellcheck disable=SC1091
(
  source "$ROOT_DIR/scripts/load-env.sh" "$TEST_DIR/no-final-newline.env"
  [[ "$KEEP" == 'original' ]]
  [[ "$SMTP_HOST" == 'host' ]]
)

# Unsafe legacy values must fail closed instead of being interpreted differently by Bash and
# Compose. configure-email.sh always emits the supported single-quoted form.
printf '%s\n' 'SMTP_PASSWORD=pa$word # comment' > "$TEST_DIR/unsafe.env"
if (source "$ROOT_DIR/scripts/load-env.sh" "$TEST_DIR/unsafe.env") 2>/dev/null; then
  printf 'unsafe unquoted dotenv value was unexpectedly accepted\n' >&2
  exit 1
fi

if command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1; then
  printf '%s\n' \
    'services:' \
    '  check:' \
    '    image: busybox' \
    '    environment:' \
    '      SMTP_HOST: ${SMTP_HOST}' \
    '      SMTP_USERNAME: ${SMTP_USERNAME}' \
    '      SMTP_PASSWORD: ${SMTP_PASSWORD}' \
    '      SMTP_FROM: ${SMTP_FROM}' > "$COMPOSE_FILE"
  docker compose --env-file "$ENV_FILE" -f "$COMPOSE_FILE" config --format json \
    > "$COMPOSE_JSON"
  command -v python3 >/dev/null 2>&1 || {
    printf 'python3 is required to verify Docker Compose round-trip output\n' >&2
    exit 1
  }
  python3 - "$COMPOSE_JSON" "$expected_host" "$expected_username" \
    "$expected_password" "$expected_from" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    environment = json.load(source)["services"]["check"]["environment"]

expected = {
    "SMTP_HOST": sys.argv[2],
    "SMTP_USERNAME": sys.argv[3],
    "SMTP_PASSWORD": sys.argv[4],
    "SMTP_FROM": sys.argv[5],
}
for key, value in environment.items():
    # Docker Compose v5 serializes literal dollar signs as `$$` in its normalized JSON
    # representation, even though the container receives the original value.
    environment[key] = value.replace("$$", "$")
if environment != expected:
    raise SystemExit("Docker Compose dotenv round-trip mismatch")
PY
else
  printf 'Docker Compose unavailable; loader round-trip passed (Compose assertion skipped).\n'
fi

printf 'dotenv round-trip checks passed.\n'
