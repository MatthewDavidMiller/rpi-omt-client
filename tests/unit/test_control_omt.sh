#!/bin/bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
CONTROL="${ROOT}/deploy/container/control-omt.sh"
RUNTIME="${ROOT}/deploy/container/runtime-lib.sh"
CASE_DIR="$(mktemp -d)"
trap 'rm -rf "${CASE_DIR}"' EXIT

mkdir -p "${CASE_DIR}/config/run"
cat > "${CASE_DIR}/receiver" <<'EOF'
#!/bin/bash
exit 1
EOF
chmod 0755 "${CASE_DIR}/receiver"

run_control() {
    OMT_CONFIG_DIR="${CASE_DIR}/config" \
    OMT_RUNTIME_LIB="${RUNTIME}" \
    OMT_RECEIVER_COMMAND="${CASE_DIR}/receiver" \
    START_OMT_CMD="${CASE_DIR}/receiver" \
        "${CONTROL}" "$@"
}

if run_control invalid >/dev/null 2>&1; then
    echo "invalid controller action was accepted" >&2
    exit 1
fi
if run_control start >/dev/null 2>&1; then
    echo "controller started without a target" >&2
    exit 1
fi

printf '%s\n' '{"schema":1,"kind":"discovered","name":"Camera"}' \
    > "${CASE_DIR}/config/source_target.json"
if run_control start >/dev/null 2>&1; then
    echo "controller trusted a receiver that exited immediately" >&2
    exit 1
fi
if run_control stop > "${CASE_DIR}/output" 2>&1; then
    echo "already-stopped controller returned success" >&2
    exit 1
fi
grep -q 'already stopped' "${CASE_DIR}/output"
if run_control status >/dev/null 2>&1; then
    echo "stopped controller returned success status" >&2
    exit 1
fi

printf '999999 1\n' > "${CASE_DIR}/config/run/omt.pid"
if run_control status >/dev/null 2>&1; then
    echo "stale PID record was trusted" >&2
    exit 1
fi
[[ ! -e "${CASE_DIR}/config/run/omt.pid" ]]

rm -f "${CASE_DIR}/config/source_target.json"
ln -s target "${CASE_DIR}/config/source_target.json"
if run_control start >/dev/null 2>&1; then
    echo "symlinked source target was trusted" >&2
    exit 1
fi

echo "OMT controller tests passed"
