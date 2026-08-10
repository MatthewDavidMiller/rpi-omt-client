#!/bin/bash
# Exercise the real Rust receiver against the real Rust sender over TCP.
set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "Usage: $0 SENDER_BINARY RECEIVER_BINARY" >&2
    exit 2
fi
sender="$1"
receiver="$2"
[[ -x "${sender}" && -x "${receiver}" ]] || {
    echo "ERROR: sender and receiver binaries must be executable" >&2
    exit 1
}

case_dir="$(mktemp -d)"
sender_pid=""
cleanup() {
    if [[ -n "${sender_pid}" ]]; then
        kill "${sender_pid}" 2>/dev/null || true
        wait "${sender_pid}" 2>/dev/null || true
    fi
    rm -rf -- "${case_dir}"
}
trap cleanup EXIT

port=""
for candidate in $(seq 6500 6600); do
    "${sender}" --bind 127.0.0.1 --port "${candidate}" \
        >"${case_dir}/sender.log" 2>&1 &
    sender_pid=$!
    sleep 0.1
    if kill -0 "${sender_pid}" 2>/dev/null; then
        port="${candidate}"
        break
    fi
    wait "${sender_pid}" 2>/dev/null || true
    sender_pid=""
done
[[ -n "${port}" ]] || {
    echo "ERROR: no sender test port was available" >&2
    exit 1
}

output="$("${receiver}" probe --target "omt://127.0.0.1:${port}" \
    --timeout-ms 8000 --json)"
for field in \
    '"ok":true' \
    '"video":true' \
    '"audio":true' \
    '"width":1920' \
    '"height":1080' \
    '"frame_rate":60.0' \
    '"channels":2' \
    '"sample_rate":48000'; do
    grep -Fq "${field}" <<<"${output}" || {
        echo "ERROR: sender/receiver probe omitted ${field}: ${output}" >&2
        exit 1
    }
done
echo "Rust sender/receiver probe passed on TCP ${port}."
