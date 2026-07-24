#!/bin/bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
START="${ROOT}/deploy/container/start-omt.sh"
CASE_DIR="$(mktemp -d)"
trap 'rm -rf "${CASE_DIR}"' EXIT
mkdir -p "${CASE_DIR}/config/run"

cat > "${CASE_DIR}/receiver" <<'EOF'
#!/bin/bash
printf 'storage=%s\n' "${OMT_STORAGE_PATH}"
printf 'args='
printf '<%s>' "$@"
printf '\n'
EOF
chmod 0755 "${CASE_DIR}/receiver"

run_start() {
    OMT_CONFIG_DIR="${CASE_DIR}/config" \
    OMT_STORAGE_PATH="${CASE_DIR}/config/omt" \
    OMT_RECEIVER_COMMAND="${CASE_DIR}/receiver" \
    OMT_HDMI_CONNECTOR="${1:-auto}" \
        "${START}"
}

printf '%s\n' '{"schema":1,"kind":"discovered","name":"Studio Camera"}' \
    > "${CASE_DIR}/config/source_target.json"
output="$(run_start HDMI-A-2)"
grep -Fq 'storage='"${CASE_DIR}"'/config/omt' <<< "${output}"
grep -Fq '<play><--target><Studio Camera><--connector><HDMI-A-2>' <<< "${output}"
grep -Fq '<--status-file><'"${CASE_DIR}"'/config/run/playback-status.json>' <<< "${output}"

printf '%s\n' '{"schema":1,"kind":"direct","uri":"omt://192.0.2.1:6400"}' \
    > "${CASE_DIR}/config/source_target.json"
run_start auto | grep -Fq '<--target><omt://192.0.2.1:6400>'

if run_start HDMI-A-3 >/dev/null 2>&1; then
    echo "invalid connector was accepted" >&2
    exit 1
fi
printf '%s\n' '{"schema":2,"kind":"discovered","name":"Camera"}' \
    > "${CASE_DIR}/config/source_target.json"
if run_start auto >/dev/null 2>&1; then
    echo "invalid target schema was accepted" >&2
    exit 1
fi
rm -f "${CASE_DIR}/config/source_target.json"
ln -s target "${CASE_DIR}/config/source_target.json"
if run_start auto >/dev/null 2>&1; then
    echo "symlinked target was accepted" >&2
    exit 1
fi

echo "OMT receiver launcher tests passed"
