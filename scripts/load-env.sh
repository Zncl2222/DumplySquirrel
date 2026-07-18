#!/usr/bin/env bash
# Load the project's Compose-compatible dotenv subset without evaluating it as shell code.
# This file is intended to be sourced from the Bash development scripts. Unquoted values are
# accepted only when they contain no interpolation/comment/quoting syntax. Single-quoted values
# are literal; ambiguous quote/backslash escapes are rejected because Compose and dotenvy do not
# interpret all of them identically.

if [ "$#" -ne 1 ]; then
  printf 'Usage: source scripts/load-env.sh ENV_FILE\n' >&2
  return 1
fi

dumply_env_file="$1"
while IFS= read -r dumply_env_line || [ -n "$dumply_env_line" ]; do
  dumply_env_line="${dumply_env_line%$'\r'}"
  case "$dumply_env_line" in
    ''|'#'*) continue ;;
  esac
  if [[ "$dumply_env_line" != *=* ]]; then
    printf 'Invalid dotenv line in %s: expected KEY=VALUE\n' "$dumply_env_file" >&2
    return 1
  fi
  dumply_env_key="${dumply_env_line%%=*}"
  dumply_env_raw_value="${dumply_env_line#*=}"
  if [[ ! "$dumply_env_key" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]]; then
    printf 'Invalid dotenv key in %s: %s\n' "$dumply_env_file" "$dumply_env_key" >&2
    return 1
  fi

  if [[ "$dumply_env_raw_value" == \'* ]]; then
    if (( ${#dumply_env_raw_value} < 2 )) ||
      [[ "${dumply_env_raw_value: -1}" != "'" ]]; then
      printf 'Invalid single-quoted dotenv value for %s in %s\n' \
        "$dumply_env_key" "$dumply_env_file" >&2
      return 1
    fi

    dumply_env_value="${dumply_env_raw_value:1:${#dumply_env_raw_value}-2}"
    if [[ "$dumply_env_value" == *"'"* || "$dumply_env_value" == *"\\"* ]]; then
      printf 'Ambiguous quote or backslash escape for %s in %s\n' \
        "$dumply_env_key" "$dumply_env_file" >&2
      return 1
    fi
  else
    if [[ "$dumply_env_raw_value" == *'$'* || "$dumply_env_raw_value" == *'#'* ||
      "$dumply_env_raw_value" == *"'"* || "$dumply_env_raw_value" == *'"'* ||
      "$dumply_env_raw_value" == *"\\"* || "$dumply_env_raw_value" =~ [[:space:]] ]]; then
      printf 'Unsafe unquoted dotenv value for %s in %s; use Compose single quotes\n' \
        "$dumply_env_key" "$dumply_env_file" >&2
      return 1
    fi
    dumply_env_value="$dumply_env_raw_value"
  fi
  export "$dumply_env_key=$dumply_env_value"
done < "$dumply_env_file"

unset dumply_env_file dumply_env_line dumply_env_key dumply_env_raw_value
unset dumply_env_value
