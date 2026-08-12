#!/bin/bash
set -euo pipefail

umask 077
OMT_CONFIG_DIR="${OMT_CONFIG_DIR:-/etc/omt}"
export HOME="${HOME:-${OMT_CONFIG_DIR}}"
export OMT_STORAGE_PATH="${OMT_STORAGE_PATH:-${OMT_CONFIG_DIR}/omt}"
export OMT_RUNTIME_DIR="${OMT_RUNTIME_DIR:-${OMT_CONFIG_DIR}/run}"
CONTROL_OMT_CMD="${CONTROL_OMT_CMD:-/usr/local/bin/control-omt.sh}"
OMT_WEB_CMD="${OMT_WEB_CMD:-/usr/local/bin/omt-web}"
WEB_PORT="${WEB_PORT:-5000}"

mkdir -p "${OMT_CONFIG_DIR}" "${OMT_STORAGE_PATH}"

# The compose file mounts a tmpfs one level above this, world-writable like any
# tmpfs mount point. Owning a 0700 directory inside it keeps the lock, PID
# record, and status file as private as they were on the config volume.
if [[ -L "${OMT_RUNTIME_DIR}" || ( -e "${OMT_RUNTIME_DIR}" && ! -d "${OMT_RUNTIME_DIR}" ) ]]; then
    echo "Unsafe OMT runtime directory: ${OMT_RUNTIME_DIR}" >&2
    exit 1
fi
mkdir -p "${OMT_RUNTIME_DIR}"
chmod 700 "${OMT_RUNTIME_DIR}"

sync_replace() {
    local staged="$1" target="$2"
    sync -f "${staged}"
    mv -fT -- "${staged}" "${target}"
    sync -d "$(dirname -- "${target}")"
}

safe_nonempty_file() {
    local path="$1" maximum="$2" size content
    [[ -f "${path}" && ! -L "${path}" ]] || return 1
    size="$(stat -c '%s' -- "${path}" 2>/dev/null || true)"
    [[ "${size}" =~ ^[1-9][0-9]*$ ]] && (( size <= maximum )) || return 1
    # Byte length alone accepts whitespace-only secrets that later crash auth
    # at import time; require at least one non-whitespace character.
    content="$(tr -d '[:space:]' < "${path}" | head -c 1 || true)"
    [[ -n "${content}" ]]
}

password="$(${OMT_WEB_CMD} initialize)"
if [[ -n "${password}" ]]; then
    printf '%s\n' "============================================"
    printf '%s\n' " Web UI password (save this now):"
    printf ' %s\n' "${password}"
    printf '%s\n' "============================================"
fi
unset password
safe_nonempty_file "${OMT_CONFIG_DIR}/web_secret" 256 || {
    echo "Rust Web secret was not initialized safely" >&2
    exit 1
}
chmod 600 "${OMT_CONFIG_DIR}/web_secret"
safe_nonempty_file "${OMT_CONFIG_DIR}/web_password" 16384 || {
    echo "Rust Web password was not initialized safely" >&2
    exit 1
}
chmod 600 "${OMT_CONFIG_DIR}/web_password"

settings_file="${OMT_STORAGE_PATH}/settings.xml"
if [[ ! -e "${settings_file}" ]]; then
    settings_tmp="$(mktemp "${OMT_STORAGE_PATH}/.settings.xml.XXXXXX")"
    trap 'rm -f -- "${settings_tmp}"' EXIT
    printf '%s\n' '<?xml version="1.0" encoding="utf-8"?>' '<Settings />' > "${settings_tmp}"
    chmod 600 "${settings_tmp}"
    sync_replace "${settings_tmp}" "${settings_file}"
    trap - EXIT
elif [[ -L "${settings_file}" || ! -f "${settings_file}" ]]; then
    echo "Unsafe OMT settings path: ${settings_file}" >&2
    exit 1
fi
chmod 600 "${settings_file}"

ssl_dir="${OMT_CONFIG_DIR}/ssl"
ssl_cert="${ssl_dir}/cert.pem"
ssl_key="${ssl_dir}/key.pem"
if [[ -L "${ssl_dir}" || ( -e "${ssl_dir}" && ! -d "${ssl_dir}" ) ]]; then
    echo "Unsafe TLS directory: ${ssl_dir}" >&2
    exit 1
fi
mkdir -p "${ssl_dir}"
chmod 700 "${ssl_dir}"

tls_valid() {
    [[ -f "${ssl_cert}" && ! -L "${ssl_cert}" &&
       -f "${ssl_key}" && ! -L "${ssl_key}" ]] || return 1
    openssl x509 -checkend 2592000 -noout -in "${ssl_cert}" >/dev/null 2>&1 || return 1
    openssl pkey -check -noout -in "${ssl_key}" >/dev/null 2>&1 || return 1
    [[ "$(openssl x509 -pubkey -noout -in "${ssl_cert}")" == \
       "$(openssl pkey -pubout -in "${ssl_key}")" ]]
}

if ! tls_valid; then
    key_tmp="$(mktemp "${ssl_dir}/.key.pem.XXXXXX")"
    cert_tmp="$(mktemp "${ssl_dir}/.cert.pem.XXXXXX")"
    trap 'rm -f -- "${key_tmp}" "${cert_tmp}"' EXIT
    openssl req -x509 -nodes -newkey ec -pkeyopt ec_paramgen_curve:P-384 \
        -days 365 -keyout "${key_tmp}" -out "${cert_tmp}" \
        -subj "/CN=omt-client" \
        -addext "subjectAltName=DNS:omt-client,DNS:localhost,IP:127.0.0.1"
    chmod 600 "${key_tmp}"
    chmod 644 "${cert_tmp}"
    sync_replace "${key_tmp}" "${ssl_key}"
    sync_replace "${cert_tmp}" "${ssl_cert}"
    trap - EXIT
fi
chmod 600 "${ssl_key}"
chmod 644 "${ssl_cert}"

if [[ -f "${OMT_CONFIG_DIR}/source_target.json" &&
      ! -L "${OMT_CONFIG_DIR}/source_target.json" ]]; then
    if ! control_output="$("${CONTROL_OMT_CMD}" start 2>&1)"; then
        echo "Warning: configured OMT receiver failed to start: ${control_output}" >&2
    else
        printf '%s\n' "${control_output}"
    fi
fi

export OMT_TLS_CERT_FILE="${ssl_cert}"
export OMT_TLS_KEY_FILE="${ssl_key}"
exec "${OMT_WEB_CMD}"
