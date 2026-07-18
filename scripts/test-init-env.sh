#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_DIR="$(mktemp -d)"
trap 'rm -rf -- "$TEST_DIR"' EXIT

mkdir -p "$TEST_DIR/project/scripts"
cp "$ROOT_DIR/.env.example" "$TEST_DIR/project/.env.example"
cp "$ROOT_DIR/scripts/init-env.sh" "$TEST_DIR/project/scripts/init-env.sh"

first_status=0
second_status=0
sh "$TEST_DIR/project/scripts/init-env.sh" > "$TEST_DIR/first.log" 2>&1 &
first_pid=$!
sh "$TEST_DIR/project/scripts/init-env.sh" > "$TEST_DIR/second.log" 2>&1 &
second_pid=$!
wait "$first_pid" || first_status=$?
wait "$second_pid" || second_status=$?

successes=0
[[ "$first_status" -eq 0 ]] && successes=$((successes + 1))
[[ "$second_status" -eq 0 ]] && successes=$((successes + 1))
if [[ "$successes" -ne 1 ]]; then
  printf 'expected exactly one concurrent init-env process to succeed; statuses: %s, %s\n' \
    "$first_status" "$second_status" >&2
  exit 1
fi

ENV_FILE="$TEST_DIR/project/.env"
[[ -f "$ENV_FILE" ]]
[[ "$(stat -c '%a' "$ENV_FILE")" == '600' ]]
! grep -q '=change_me_' "$ENV_FILE"
for required_name in DB_PASSWORD JWT_SECRET DATABASE_ENCRYPTION_KEY ADMIN_PASSWORD DATABASE_URL; do
  grep -Eq "^${required_name}=.+" "$ENV_FILE"
done

checksum_before="$(cksum "$ENV_FILE")"
if sh "$TEST_DIR/project/scripts/init-env.sh" >/dev/null 2>&1; then
  printf 'init-env unexpectedly overwrote an existing .env\n' >&2
  exit 1
fi
[[ "$(cksum "$ENV_FILE")" == "$checksum_before" ]]

printf 'init-env atomic no-clobber checks passed.\n'
