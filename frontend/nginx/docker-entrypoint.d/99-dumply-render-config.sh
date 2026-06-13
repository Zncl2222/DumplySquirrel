#!/bin/sh
set -eu

if [ "${ENABLE_TLS:-false}" = "true" ]; then
  template="/etc/nginx/custom-templates/https.conf.tmpl"
else
  template="/etc/nginx/custom-templates/http.conf.tmpl"
fi

envsubst '${NGINX_PORT} ${NGINX_HTTPS_PORT} ${SERVER_NAME} ${TLS_CERT_FILE} ${TLS_KEY_FILE}' \
  < "$template" \
  > /etc/nginx/conf.d/default.conf
