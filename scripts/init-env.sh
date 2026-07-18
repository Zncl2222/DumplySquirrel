#!/usr/bin/env sh
set -eu
umask 077

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
ENV_FILE="$ROOT_DIR/.env"
TEMPLATE_FILE="$ROOT_DIR/.env.example"
TEMP_FILE=""

cleanup() {
  if [ -n "$TEMP_FILE" ] && [ -f "$TEMP_FILE" ]; then
    rm -f "$TEMP_FILE"
  fi
}
trap cleanup EXIT HUP INT TERM

if [ -e "$ENV_FILE" ]; then
  printf 'Refusing to overwrite existing %s\n' "$ENV_FILE" >&2
  exit 1
fi

random_hex() {
  bytes="$1"
  if command -v openssl >/dev/null 2>&1; then
    openssl rand -hex "$bytes"
  else
    od -An -N "$bytes" -tx1 /dev/urandom | tr -d ' \n'
  fi
}

db_password=$(random_hex 24)
jwt_secret=$(random_hex 32)
encryption_key=$(random_hex 32)
admin_password=$(random_hex 24)

TEMP_FILE=$(mktemp "${ENV_FILE}.tmp.XXXXXX")
sed \
  -e "s|^DB_PASSWORD=.*|DB_PASSWORD=${db_password}|" \
  -e "s|^DB_PASSWORD_URL_ENCODED=.*|DB_PASSWORD_URL_ENCODED=|" \
  -e "s|^JWT_SECRET=.*|JWT_SECRET=${jwt_secret}|" \
  -e "s|^DATABASE_ENCRYPTION_KEY=.*|DATABASE_ENCRYPTION_KEY=${encryption_key}|" \
  -e "s|^ADMIN_PASSWORD=.*|ADMIN_PASSWORD=${admin_password}|" \
  -e "s|^DATABASE_URL=.*|DATABASE_URL=postgres://dumply:${db_password}@127.0.0.1:5432/dumply_squirrel_dev|" \
  -e "s|^DEV_ALLOW_INSECURE_SECRETS=.*|DEV_ALLOW_INSECURE_SECRETS=false|" \
  "$TEMPLATE_FILE" > "$TEMP_FILE"

if grep -q '=change_me_' "$TEMP_FILE"; then
  printf 'Generated environment still contains a public placeholder; refusing to install it.\n' >&2
  exit 1
fi
for required_name in DB_PASSWORD JWT_SECRET DATABASE_ENCRYPTION_KEY ADMIN_PASSWORD DATABASE_URL; do
  if ! grep -Eq "^${required_name}=.+" "$TEMP_FILE"; then
    printf 'Generated environment is missing %s; refusing to install it.\n' "$required_name" >&2
    exit 1
  fi
done
chmod 600 "$TEMP_FILE"
# A hard-link install is an atomic no-clobber operation because the temporary file lives in the
# same directory. The earlier existence check is only an optimization; this final link closes the
# race between two concurrent initializers without exposing a partially written .env.
if ! ln "$TEMP_FILE" "$ENV_FILE" 2>/dev/null; then
  printf 'Refusing to overwrite existing %s\n' "$ENV_FILE" >&2
  exit 1
fi
rm -f "$TEMP_FILE"
TEMP_FILE=""

printf 'Created %s with unique secrets and mode 0600.\n' "$ENV_FILE"
printf 'Store this file securely; DATABASE_ENCRYPTION_KEY is required to decrypt saved target URLs.\n'
printf 'Use ADMIN_USERNAME and ADMIN_PASSWORD from this file for the first login.\n'
