#!/bin/bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
ENTRYPOINT="${ROOT}/deploy/container/entrypoint.sh"
CASE_DIR="$(mktemp -d)"
trap 'rm -rf "${CASE_DIR}"' EXIT
mkdir -p "${CASE_DIR}/config"

cat > "${CASE_DIR}/omt-web" <<'EOF'
#!/bin/bash
if [[ "${1:-}" == initialize ]]; then
    exec "${REAL_OMT_WEB}" initialize
fi
printf '%s\n' "$*" > "${ENTRYPOINT_WEB_RECORD}"
EOF
cat > "${CASE_DIR}/control" <<'EOF'
#!/bin/bash
printf '%s\n' "$*" > "${ENTRYPOINT_CONTROL_RECORD}"
EOF
chmod 0755 "${CASE_DIR}/omt-web" "${CASE_DIR}/control"

run_entrypoint() {
    OMT_CONFIG_DIR="${CASE_DIR}/config" \
    OMT_STORAGE_PATH="${CASE_DIR}/config/omt" \
    OMT_WEB_CMD="${CASE_DIR}/omt-web" \
    CONTROL_OMT_CMD="${CASE_DIR}/control" \
    REAL_OMT_WEB="${ROOT}/target/debug/omt-web" \
    ENTRYPOINT_WEB_RECORD="${CASE_DIR}/web.args" \
    ENTRYPOINT_CONTROL_RECORD="${CASE_DIR}/control.args" \
    WEB_PORT=5443 \
        "${ENTRYPOINT}"
}

run_entrypoint >/dev/null
[[ -s "${CASE_DIR}/config/web_secret" ]]
[[ -s "${CASE_DIR}/config/web_password" ]]
[[ "$(stat -c '%a' "${CASE_DIR}/config/web_secret")" == 600 ]]
[[ "$(stat -c '%a' "${CASE_DIR}/config/web_password")" == 600 ]]
[[ -s "${CASE_DIR}/config/ssl/key.pem" ]]
[[ -s "${CASE_DIR}/config/ssl/cert.pem" ]]
grep -q '<Settings />' "${CASE_DIR}/config/omt/settings.xml"
[[ -f "${CASE_DIR}/web.args" ]]
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
rm -f "${CASE_DIR}/config/omt/settings.xml"

run_entrypoint_with_runtime() {
    OMT_CONFIG_DIR="${CASE_DIR}/config" \
    OMT_STORAGE_PATH="${CASE_DIR}/config/omt" \
    OMT_RUNTIME_DIR="$1" \
    OMT_WEB_CMD="${CASE_DIR}/omt-web" \
    CONTROL_OMT_CMD="${CASE_DIR}/control" \
    REAL_OMT_WEB="${ROOT}/target/debug/omt-web" \
    ENTRYPOINT_WEB_RECORD="${CASE_DIR}/web.args" \
    ENTRYPOINT_CONTROL_RECORD="${CASE_DIR}/control.args" \
    WEB_PORT=5443 \
        "${ENTRYPOINT}"
}

# The tmpfs mount point is world-writable like any tmpfs. The entrypoint has to
# own a private directory inside it, or the lock, PID record, and status file
# would be less protected than they were on the config volume.
RUNTIME_DIR="${CASE_DIR}/runtime/state"
run_entrypoint_with_runtime "${RUNTIME_DIR}" >/dev/null 2>&1
[[ -d "${RUNTIME_DIR}" ]]
[[ "$(stat -c '%a' "${RUNTIME_DIR}")" == 700 ]]

# Without the tmpfs mount the runtime directory falls back inside the config
# volume, and the entrypoint must not then delete the directory it is using.
run_entrypoint >/dev/null 2>&1
[[ -d "${CASE_DIR}/config/run" ]]
[[ "$(stat -c '%a' "${CASE_DIR}/config/run")" == 700 ]]

# When runtime is on tmpfs, an upgrade leftover under the config volume is
# dropped rather than chmod-preserved as a second write path.
mkdir -p "${CASE_DIR}/config/run"
printf leftover > "${CASE_DIR}/config/run/stale"
run_entrypoint_with_runtime "${CASE_DIR}/runtime/state" >/dev/null 2>&1
[[ ! -e "${CASE_DIR}/config/run" ]]
[[ -d "${CASE_DIR}/runtime/state" ]]

# A hostile runtime path is refused rather than written through.
rm -rf "${CASE_DIR}/runtime"
mkdir -p "${CASE_DIR}/runtime"
ln -s /etc "${CASE_DIR}/runtime/state"
if run_entrypoint_with_runtime "${CASE_DIR}/runtime/state" >/dev/null 2>&1; then
    echo "entrypoint accepted a symlinked runtime directory" >&2
    exit 1
fi

# Whitespace-only secrets are not usable credentials; regenerate them.
printf '   \n' > "${CASE_DIR}/config/web_secret"
chmod 600 "${CASE_DIR}/config/web_secret"
BEFORE_SECRET="$(stat -c '%i %s' -- "${CASE_DIR}/config/web_secret")"
run_entrypoint >/dev/null 2>&1
AFTER_SECRET="$(stat -c '%i %s' -- "${CASE_DIR}/config/web_secret")"
[[ "${BEFORE_SECRET}" != "${AFTER_SECRET}" ]]
CONTENT="$(tr -d '[:space:]' < "${CASE_DIR}/config/web_secret")"
[[ -n "${CONTENT}" ]]

echo "OMT entrypoint tests passed"
