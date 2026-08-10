#!/bin/bash
# Bounded lifecycle wrapper for the developer-side Rust OMT test sender.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
SENDER_HOME="${OMT_TEST_SENDER_HOME:-${PROJECT_ROOT}/.build/omt-test-sender}"
CURRENT="${SENDER_HOME}/current"
RUNTIME="${SENDER_HOME}/runtime"
PID_FILE="${RUNTIME}/sender.pid"
LOG_FILE="${RUNTIME}/sender.log"
PORT_FILE="${RUNTIME}/sender.port"
PID_START_ATTEMPTS=50
PID_START_INTERVAL=0.1
LOG_MAX_BYTES=$((1024 * 1024))
LOG_KEEP_BYTES=$((512 * 1024))

usage() {
    echo "Usage: $0 {start|stop|status|run|logs}" >&2
}

sender_binary() {
    readlink -f "${CURRENT}/bin/omt-test-sender" 2>/dev/null || true
}

process_start_time() {
    local pid="$1"
    awk '{ print $22 }' "/proc/${pid}/stat" 2>/dev/null
}

process_matches() {
    local pid="$1"
    local expected_start="$2"
    local binary="$3"
    local argument
    [[ -n "${pid}" && -n "${expected_start}" && -n "${binary}" ]] || return 1
    kill -0 "${pid}" 2>/dev/null || return 1
    [[ "$(process_start_time "${pid}")" == "${expected_start}" ]] || return 1
    [[ "$(readlink -f "/proc/${pid}/exe" 2>/dev/null || true)" == "${binary}" ]] && return 0
    while IFS= read -r argument; do
        [[ "${argument}" == "${binary}" ]] && return 0
    done < <({ tr '\0' '\n' < "/proc/${pid}/cmdline"; } 2>/dev/null || true)
    return 1
}

read_record() {
    local pid start extra
    [[ -f "${PID_FILE}" && ! -L "${PID_FILE}" ]] || return 1
    read -r pid start extra < "${PID_FILE}" || return 1
    [[ -z "${extra:-}" && "${pid:-}" =~ ^[1-9][0-9]*$ &&
       "${start:-}" =~ ^[1-9][0-9]*$ ]] || return 1
    printf '%s %s\n' "${pid}" "${start}"
}

active_record() {
    local record pid start binary
    record="$(read_record 2>/dev/null || true)"
    [[ -n "${record}" ]] || return 1
    read -r pid start <<< "${record}"
    binary="$(sender_binary)"
    process_matches "${pid}" "${start}" "${binary}" || return 1
    printf '%s\n' "${record}"
}

listening_port() {
    local pid="$1"
    command -v ss >/dev/null 2>&1 || return 1
    ss -H -ltnp 2>/dev/null | awk -v needle="pid=${pid}," '
        index($0, needle) {
            local_address = $4
            sub(/^.*:/, "", local_address)
            if (local_address ~ /^6[45][0-9][0-9]$/) {
                print local_address
                exit
            }
        }
    '
}

sender_address() {
    local address
    address="$(ip -4 route get 1.1.1.1 2>/dev/null | awk '
        {
            for (field = 1; field < NF; field++) {
                if ($field == "src") {
                    print $(field + 1)
                    exit
                }
            }
        }
    ')"
    if [[ -z "${address}" ]]; then
        address="$(hostname -i 2>/dev/null | awk '{ print $1; exit }')"
    fi
    [[ -n "${address}" ]] || address="127.0.0.1"
    printf '%s\n' "${address}"
}

require_build() {
    local binary
    binary="$(sender_binary)"
    [[ -n "${binary}" && -x "${binary}" ]] || {
        echo "ERROR: OMT test sender is not built; run make build-omt-sender" >&2
        exit 1
    }
}

rotate_log() {
    local path="$1"
    local size stage
    [[ -f "${path}" && ! -L "${path}" ]] || return 0
    size="$(stat -c '%s' "${path}" 2>/dev/null || true)"
    [[ "${size}" =~ ^[0-9]+$ ]] || return 0
    ((size <= LOG_MAX_BYTES)) && return 0
    stage="${path}.tmp.$$"
    tail -c "${LOG_KEEP_BYTES}" "${path}" > "${stage}"
    chmod 0600 "${stage}"
    mv -f -- "${stage}" "${path}"
}

prepare_log() {
    [[ ! -L "${LOG_FILE}" ]] || {
        echo "ERROR: refusing symlink sender log: ${LOG_FILE}" >&2
        return 1
    }
    touch "${LOG_FILE}"
    chmod 0600 "${LOG_FILE}"
}

start_sender() {
    local binary pid start port attempt record
    require_build
    record="$(active_record 2>/dev/null || true)"
    if [[ -n "${record}" ]]; then
        echo "ERROR: OMT test sender is already running (pid ${record%% *})" >&2
        return 3
    fi
    mkdir -p "${RUNTIME}"
    rm -f -- "${PID_FILE}" "${PORT_FILE}"
    rotate_log "${LOG_FILE}"
    prepare_log
    binary="$(sender_binary)"
    (
        cd "${RUNTIME}"
        exec nohup "${binary}"
    ) >> "${LOG_FILE}" 2>&1 </dev/null &
    pid=$!
    start=""
    for attempt in $(seq 1 "${PID_START_ATTEMPTS}"); do
        start="$(process_start_time "${pid}" 2>/dev/null || true)"
        [[ -n "${start}" ]] && break
        sleep "${PID_START_INTERVAL}"
    done
    [[ -n "${start}" ]] || {
        wait "${pid}" 2>/dev/null || true
        echo "ERROR: OMT test sender exited during startup" >&2
        return 1
    }
    printf '%s %s\n' "${pid}" "${start}" > "${PID_FILE}"
    chmod 0600 "${PID_FILE}"
    port=""
    for attempt in $(seq 1 "${PID_START_ATTEMPTS}"); do
        process_matches "${pid}" "${start}" "${binary}" || break
        port="$(listening_port "${pid}" 2>/dev/null || true)"
        [[ -n "${port}" ]] && break
        sleep "${PID_START_INTERVAL}"
    done
    if [[ -z "${port}" ]]; then
        kill "${pid}" 2>/dev/null || true
        wait "${pid}" 2>/dev/null || true
        rm -f -- "${PID_FILE}"
        echo "ERROR: OMT test sender did not listen on the OMT port range" >&2
        return 1
    fi
    printf '%s\n' "${port}" > "${PORT_FILE}"
    chmod 0600 "${PORT_FILE}"
    echo "OMT test sender running: omt://$(sender_address):${port} (pid ${pid})"
}

stop_sender() {
    local record pid start binary attempt
    record="$(active_record 2>/dev/null || true)"
    if [[ -z "${record}" ]]; then
        rm -f -- "${PID_FILE}" "${PORT_FILE}"
        echo "OMT test sender is stopped"
        return 0
    fi
    read -r pid start <<< "${record}"
    binary="$(sender_binary)"
    kill "${pid}"
    for attempt in $(seq 1 50); do
        if ! process_matches "${pid}" "${start}" "${binary}"; then
            rm -f -- "${PID_FILE}" "${PORT_FILE}"
            echo "Stopped OMT test sender"
            return 0
        fi
        sleep 0.1
    done
    kill -KILL "${pid}" 2>/dev/null || true
    for attempt in $(seq 1 50); do
        if ! process_matches "${pid}" "${start}" "${binary}"; then
            rm -f -- "${PID_FILE}" "${PORT_FILE}"
            echo "Force-stopped OMT test sender"
            return 0
        fi
        sleep 0.1
    done
    echo "ERROR: OMT test sender ${pid} survived SIGKILL" >&2
    return 1
}

status_sender() {
    local record port
    record="$(active_record 2>/dev/null || true)"
    if [[ -z "${record}" ]]; then
        echo "stopped"
        return 3
    fi
    port="$(listening_port "${record%% *}" 2>/dev/null || true)"
    [[ -n "${port}" ]] || port="unknown"
    echo "running pid=${record%% *} port=${port}"
}

run_sender() {
    local binary
    require_build
    binary="$(sender_binary)"
    exec "${binary}"
}

action="${1:-}"
[[ $# -eq 1 ]] || {
    usage
    exit 2
}
case "${action}" in
    start) start_sender ;;
    stop) stop_sender ;;
    status) status_sender ;;
    run) run_sender ;;
    logs)
        mkdir -p "${RUNTIME}"
        prepare_log
        exec tail -n 100 -f "${LOG_FILE}"
        ;;
    *)
        usage
        exit 2
        ;;
esac
