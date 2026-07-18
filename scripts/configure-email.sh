#!/usr/bin/env sh
set -eu
umask 077

ENV_FILE="${1:-.env}"
TEMP_FILE=""
STTY_STATE=""

cleanup() {
  if [ -n "$STTY_STATE" ]; then
    stty "$STTY_STATE" 2>/dev/null || true
    STTY_STATE=""
  fi
  if [ -n "$TEMP_FILE" ] && [ -f "$TEMP_FILE" ]; then
    rm -f "$TEMP_FILE"
  fi
}
trap cleanup EXIT HUP INT TERM

prompt() {
  label="$1"
  default="$2"
  if [ -n "$default" ]; then
    printf '%s [%s]: ' "$label" "$default"
  else
    printf '%s: ' "$label"
  fi
  IFS= read -r value || value=""
  if [ -z "$value" ]; then
    value="$default"
  fi
  PROMPT_VALUE="$value"
}

prompt_secret() {
  label="$1"
  printf '%s: ' "$label"
  if [ -t 0 ]; then
    STTY_STATE=$(stty -g)
    stty -echo
  fi
  IFS= read -r value || value=""
  if [ -n "$STTY_STATE" ]; then
    stty "$STTY_STATE"
    STTY_STATE=""
    printf '\n'
  fi
  PROMPT_VALUE="$value"
}

validate_dotenv_value() {
  name="$1"
  value="$2"
  # Compose applies interpolation to unquoted and double-quoted dotenv values. Store values in
  # single quotes so credentials containing `$`, `${...}`, `#`, or spaces remain literal. Compose
  # and dotenvy differ on escaped characters inside single quotes, so reject that ambiguous edge
  # instead of silently producing different production and host-dev credentials.
  case "$value" in
    *"'"*|*"\\"*)
      printf '%s cannot contain single quotes or backslashes in this dotenv format\n' "$name" >&2
      return 1
      ;;
  esac
  if printf '%s' "$value" | LC_ALL=C grep -q '[[:cntrl:]]'; then
    printf '%s cannot contain control characters in this dotenv format\n' "$name" >&2
    return 1
  fi
}

printf 'Configure DumplySquirrel SMTP settings in %s\n' "$ENV_FILE"
prompt "SMTP host" ""
smtp_host="$PROMPT_VALUE"
prompt "SMTP port" "587"
smtp_port="$PROMPT_VALUE"
prompt "SMTP username" ""
smtp_username="$PROMPT_VALUE"
prompt_secret "SMTP password"
smtp_password="$PROMPT_VALUE"
prompt "From address" ""
smtp_from="$PROMPT_VALUE"
prompt "TLS mode (starttls or none)" "starttls"
smtp_tls="$PROMPT_VALUE"

validate_dotenv_value SMTP_HOST "$smtp_host"
validate_dotenv_value SMTP_PORT "$smtp_port"
validate_dotenv_value SMTP_USERNAME "$smtp_username"
validate_dotenv_value SMTP_PASSWORD "$smtp_password"
validate_dotenv_value SMTP_FROM "$smtp_from"
validate_dotenv_value SMTP_TLS "$smtp_tls"

TEMP_FILE=$(mktemp "${ENV_FILE}.tmp.XXXXXX")
if [ -f "$ENV_FILE" ]; then
  input_file="$ENV_FILE"
else
  input_file=/dev/null
fi

# Render every answer into one adjacent temporary file and replace .env only after all prompts
# and validation succeed. awk also normalizes a missing final newline before appending new keys.
awk \
  -v smtp_host="'$smtp_host'" \
  -v smtp_port="'$smtp_port'" \
  -v smtp_username="'$smtp_username'" \
  -v smtp_password="'$smtp_password'" \
  -v smtp_from="'$smtp_from'" \
  -v smtp_tls="'$smtp_tls'" \
  '
  BEGIN {
    order[1] = "SMTP_HOST"
    order[2] = "SMTP_PORT"
    order[3] = "SMTP_USERNAME"
    order[4] = "SMTP_PASSWORD"
    order[5] = "SMTP_FROM"
    order[6] = "SMTP_TLS"
    values["SMTP_HOST"] = smtp_host
    values["SMTP_PORT"] = smtp_port
    values["SMTP_USERNAME"] = smtp_username
    values["SMTP_PASSWORD"] = smtp_password
    values["SMTP_FROM"] = smtp_from
    values["SMTP_TLS"] = smtp_tls
  }
  {
    line = $0
    key = line
    sub(/=.*/, "", key)
    if (key in values) {
      print key "=" values[key]
      seen[key] = 1
    } else {
      print line
    }
  }
  END {
    for (i = 1; i <= 6; i++) {
      key = order[i]
      if (!(key in seen)) {
        print key "=" values[key]
      }
    }
  }
  ' "$input_file" > "$TEMP_FILE"

chmod 600 "$TEMP_FILE"
mv "$TEMP_FILE" "$ENV_FILE"
TEMP_FILE=""

printf 'SMTP settings updated in %s\n' "$ENV_FILE"
