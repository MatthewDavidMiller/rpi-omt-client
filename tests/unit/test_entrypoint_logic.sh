#!/bin/bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
ENTRYPOINT="${ROOT}/omt/entrypoint.sh"
CASE_DIR="$(mktemp -d)"
trap 'rm -rf "${CASE_DIR}"' EXIT
mkdir -p "${CASE_DIR}/config"

cat > "${CASE_DIR}/gunicorn" <<'EOF'
#!/bin/bash
printf '%s\n' "$*" > "${ENTRYPOINT_GUNICORN_RECORD}"
EOF
cat > "${CASE_DIR}/control" <<'EOF'
#!/bin/bash
printf '%s\n' "$*" > "${ENTRYPOINT_CONTROL_RECORD}"
EOF
chmod 0755 "${CASE_DIR}/gunicorn" "${CASE_DIR}/control"

run_entrypoint() {
    OMT_CONFIG_DIR="${CASE_DIR}/config" \
    OMT_STORAGE_PATH="${CASE_DIR}/config/omt" \
    GUNICORN_CMD="${CASE_DIR}/gunicorn" \
    CONTROL_OMT_CMD="${CASE_DIR}/control" \
    ENTRYPOINT_GUNICORN_RECORD="${CASE_DIR}/gunicorn.args" \
    ENTRYPOINT_CONTROL_RECORD="${CASE_DIR}/control.args" \
    WEB_PORT=5443 \
        "${ENTRYPOINT}"
}

run_entrypoint >/dev/null
[[ -s "${CASE_DIR}/config/flask_secret" ]]
[[ -s "${CASE_DIR}/config/web_password" ]]
[[ "$(stat -c '%a' "${CASE_DIR}/config/flask_secret")" == 600 ]]
[[ "$(stat -c '%a' "${CASE_DIR}/config/web_password")" == 600 ]]
[[ -s "${CASE_DIR}/config/ssl/key.pem" ]]
[[ -s "${CASE_DIR}/config/ssl/cert.pem" ]]
grep -q '<Settings />' "${CASE_DIR}/config/omt/settings.xml"
grep -q -- '--bind 0.0.0.0:5443' "${CASE_DIR}/gunicorn.args"
[[ ! -e "${CASE_DIR}/control.args" ]]

printf '%s\n' '{"schema":1,"kind":"discovered","name":"Camera"}' \
    > "${CASE_DIR}/config/source_target.json"
run_entrypoint >/dev/null 2>&1
grep -qx 'start' "${CASE_DIR}/control.args"

rm -f "${CASE_DIR}/config/omt/settings.xml"
ln -s /etc/passwd "${CASE_DIR}/config/omt/settings.xml"
if run_entrypoint >/dev/null 2>&1; then
    echo "entrypoint accepted a symlinked OMT settings file" >&2
    exit 1
fi

echo "OMT entrypoint tests passed"
