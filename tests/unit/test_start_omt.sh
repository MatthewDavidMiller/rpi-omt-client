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
    PATH="${ROOT}/target/debug:${PATH}" \
        "${START}"
}

printf '%s\n' '{"schema":1,"kind":"discovered","name":"Studio Camera"}' \
    > "${CASE_DIR}/config/source_target.json"
output="$(run_start HDMI-A-2)"
grep -Fq 'storage='"${CASE_DIR}"'/config/omt' <<< "${output}"
grep -Fq '<play><--target><Studio Camera><--connector><HDMI-A-2>' <<< "${output}"
grep -Fq '<--status-file><'"${CASE_DIR}"'/config/run/playback-status.json>' <<< "${output}"

# The shipped image sets OMT_RUNTIME_DIR to a tmpfs path so the continuously
# rewritten status file never touches the SD-card-backed config volume. The
# launcher has to follow it there rather than keep its own derived default.
mkdir -p "${CASE_DIR}/runtime"
OMT_RUNTIME_DIR="${CASE_DIR}/runtime" run_start auto |
    grep -Fq '<--status-file><'"${CASE_DIR}"'/runtime/playback-status.json>'

printf '%s\n' '{"schema":1,"kind":"direct","uri":"omt://192.0.2.1:6400"}' \
    > "${CASE_DIR}/config/source_target.json"
run_start auto | grep -Fq '<--target><omt://192.0.2.1:6400>'

# ─── Decode ceiling ──────────────────────────────────────────────────────────

# With no override the board's ceiling reaches the receiver unchanged. The
# installer writes OMT_VIDEO_CEILING from the detected board, so a launcher
# that dropped it would silently run every board at the Pi 5 default.
OMT_VIDEO_CEILING='1920x1080@30,1280x720@60' run_start auto |
    grep -Fq '<--video-ceiling><1920x1080@30,1280x720@60>'

# A saved override replaces the board default.
printf '%s\n' '{"schema":1,"ceiling":"1280x720@30"}' \
    > "${CASE_DIR}/config/video_ceiling.json"
OMT_VIDEO_CEILING='1920x1080@60' run_start auto |
    grep -Fq '<--video-ceiling><1280x720@30>'

# A corrupt override fails the launch rather than falling back to a ceiling
# nobody chose: the same rule the source record already follows.
printf '%s\n' '{"schema":1,"ceiling":"3840x2160@60"}' \
    > "${CASE_DIR}/config/video_ceiling.json"
if run_start auto >/dev/null 2>&1; then
    echo "out-of-range saved ceiling was accepted" >&2
    exit 1
fi
printf '%s\n' '{"schema":2,"ceiling":"1280x720@30"}' \
    > "${CASE_DIR}/config/video_ceiling.json"
if run_start auto >/dev/null 2>&1; then
    echo "invalid ceiling schema was accepted" >&2
    exit 1
fi
rm -f "${CASE_DIR}/config/video_ceiling.json"

# An unparseable board default is a failure too. It arrives from the installer
# through the container environment, and failing here names the cause.
if OMT_VIDEO_CEILING='not-a-ceiling' run_start auto >/dev/null 2>&1; then
    echo "invalid board ceiling was accepted" >&2
    exit 1
fi

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

# Shared validators must reject hand-edited volume state the web UI would refuse.
printf '%s\n' '{"schema":1,"kind":"discovered","name":"bad\nname"}' \
    > "${CASE_DIR}/config/source_target.json"
if run_start auto >/dev/null 2>&1; then
    echo "control-character source name was accepted" >&2
    exit 1
fi
printf '%s\n' '{"schema":1,"kind":"direct","uri":"omt://not a host:6400"}' \
    > "${CASE_DIR}/config/source_target.json"
if run_start auto >/dev/null 2>&1; then
    echo "invalid direct target was accepted" >&2
    exit 1
fi

echo "OMT receiver launcher tests passed"
