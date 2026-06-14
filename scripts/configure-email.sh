#!/usr/bin/env sh
set -eu

ENV_FILE="${1:-.env}"

prompt() {
  name="$1"
  label="$2"
  default="$3"
  if [ -n "$default" ]; then
    printf '%s [%s]: ' "$label" "$default"
  else
    printf '%s: ' "$label"
  fi
  IFS= read -r value || value=""
  if [ -z "$value" ]; then
    value="$default"
  fi
  set_env "$name" "$value"
}

set_env() {
  name="$1"
  value="$2"
  escaped=$(printf '%s' "$value" | sed 's/[\\&|]/\\&/g')
  if [ -f "$ENV_FILE" ] && grep -q "^${name}=" "$ENV_FILE"; then
    tmp="${ENV_FILE}.tmp"
    sed "s|^${name}=.*|${name}=${escaped}|" "$ENV_FILE" > "$tmp"
    mv "$tmp" "$ENV_FILE"
  else
    printf '%s=%s\n' "$name" "$value" >> "$ENV_FILE"
  fi
}

if [ ! -f "$ENV_FILE" ]; then
  touch "$ENV_FILE"
fi

printf 'Configure DumplySquirrel SMTP settings in %s\n' "$ENV_FILE"
prompt SMTP_HOST "SMTP host" ""
prompt SMTP_PORT "SMTP port" "587"
prompt SMTP_USERNAME "SMTP username" ""
prompt SMTP_PASSWORD "SMTP password" ""
prompt SMTP_FROM "From address" ""
prompt SMTP_TLS "TLS mode (starttls or none)" "starttls"

printf 'SMTP settings updated in %s\n' "$ENV_FILE"
