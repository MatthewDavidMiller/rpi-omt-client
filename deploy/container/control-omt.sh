#!/bin/bash
set -euo pipefail

export LC_ALL=C
umask 077

OMT_RUNTIME_LIB="${OMT_RUNTIME_LIB:-$(dirname -- "${BASH_SOURCE[0]}")/runtime-lib.sh}"
# shellcheck source=runtime-lib.sh
source "${OMT_RUNTIME_LIB}"

OMT_CONFIG_DIR="${OMT_CONFIG_DIR:-/etc/omt}"
# Per-boot state only. The shipped image points this at a tmpfs so the
# continuously rewritten status file does not wear the SD card.
RUN_DIR="${OMT_RUNTIME_DIR:-${OMT_CONFIG_DIR}/run}"
SOURCE_FILE="${OMT_SOURCE_TARGET_FILE:-${OMT_CONFIG_DIR}/source_target.json}"
LOCK_FILE="${RUN_DIR}/control.lock"
PID_FILE="${RUN_DIR}/omt.pid"
STATUS_FILE="${OMT_PLAYBACK_STATUS_FILE:-${RUN_DIR}/playback-status.json}"
# The log stays on the persistent volume: it is the only record of why playback
# failed before a restart, and an unbounded append belongs nowhere near a small
# tmpfs.
LOG_FILE="${OMT_CONFIG_DIR}/receiver.log"
START_OMT_CMD="${START_OMT_CMD:-/usr/local/bin/start-omt.sh}"
OMT_RECEIVER_COMMAND="${OMT_RECEIVER_COMMAND:-/usr/local/bin/omt-receiver}"
PID_FILE_MAX_BYTES=128

mkdir -p "${RUN_DIR}"

read_pid_record() {
    local record pid start_time extra size
    [[ -f "${PID_FILE}" && ! -L "${PID_FILE}" ]] || return 1
    size="$(stat -c '%s' -- "${PID_FILE}" 2>/dev/null || true)"
    [[ "${size}" =~ ^[1-9][0-9]*$ ]] && (( size <= PID_FILE_MAX_BYTES )) || return 1
    IFS= read -r record < "${PID_FILE}" || return 1
    read -r pid start_time extra <<< "${record}"
    [[ -z "${extra:-}" && "${pid:-}" =~ ^[1-9][0-9]*$ &&
       "${start_time:-}" =~ ^[1-9][0-9]*$ ]] || return 1
    printf '%s %s\n' "${pid}" "${start_time}"
}

managed_process_is_valid() {
    local pid="$1" expected_start="$2" actual_start
    kill -0 "${pid}" 2>/dev/null || return 1
    actual_start="$(omt_proc_start_time "${pid}" 2>/dev/null || true)"
    [[ "${actual_start}" == "${expected_start}" ]] || return 1
    omt_process_matches_command "${pid}" "${OMT_RECEIVER_COMMAND}"
}

get_managed_record() {
    local record pid start_time
    record="$(read_pid_record 2>/dev/null || true)"
    [[ -n "${record}" ]] || return 1
    read -r pid start_time <<< "${record}"
    managed_process_is_valid "${pid}" "${start_time}" || return 1
    printf '%s %s\n' "${pid}" "${start_time}"
}

stop_locked() {
    local record pid start_time attempt
    record="$(get_managed_record 2>/dev/null || true)"
    if [[ -z "${record}" ]]; then
        rm -f -- "${PID_FILE}" "${STATUS_FILE}"
        echo "OMT receiver already stopped"
        return 3
    fi
    read -r pid start_time <<< "${record}"
    kill "${pid}" 2>/dev/null || true
    for attempt in $(seq 1 50); do
        if ! managed_process_is_valid "${pid}" "${start_time}"; then
            rm -f -- "${PID_FILE}" "${STATUS_FILE}"
            echo "Stopped OMT receiver"
            return 0
        fi
        sleep 0.1
    done
    kill -KILL "${pid}" 2>/dev/null || true
    # SIGKILL is delivered, not completed. Until the kernel finishes tearing the
    # process down it still holds /dev/dri, which DRM hands to one master at a
    # time. Reporting success here would let `restart` launch a receiver that
    # cannot open the display and drops straight into its retry loop, so keep
    # the PID record and fail instead of guessing.
    for attempt in $(seq 1 50); do
        if ! managed_process_is_valid "${pid}" "${start_time}"; then
            rm -f -- "${PID_FILE}" "${STATUS_FILE}"
            echo "Force-stopped OMT receiver"
            return 0
        fi
        sleep 0.1
    done
    echo "OMT receiver ${pid} survived SIGKILL" >&2
    return 1
}

start_locked() {
    local pid start_time attempt record tmp
    [[ -f "${SOURCE_FILE}" && ! -L "${SOURCE_FILE}" ]] || {
        echo "No safe OMT source target is configured" >&2
        return 1
    }
    record="$(get_managed_record 2>/dev/null || true)"
    if [[ -n "${record}" ]]; then
        echo "OMT receiver already running:${record%% *}"
        return 0
    fi
    rm -f -- "${PID_FILE}" "${STATUS_FILE}"
    # 9>&- closes the controller's lock descriptor in the receiver. A flock is
    # held by the open file description, not by the process that took it, so a
    # receiver that inherited fd 9 would keep this exclusive lock for as long as
    # it runs. Every later start, stop, restart, or status would then block on
    # `flock 9` until the web worker's own command timeout expired and reported
    # the action as failed -- with playback itself running perfectly.
    setsid "${START_OMT_CMD}" >>"${LOG_FILE}" 2>&1 </dev/null 9>&- &
    pid=$!
    start_time=""
    for attempt in $(seq 1 50); do
        if ! kill -0 "${pid}" 2>/dev/null; then
            break
        fi
        start_time="$(omt_proc_start_time "${pid}" 2>/dev/null || true)"
        if [[ -n "${start_time}" ]] &&
           omt_process_matches_command "${pid}" "${OMT_RECEIVER_COMMAND}"; then
            break
        fi
        sleep 0.02
    done
    if [[ -z "${start_time}" ]] ||
       ! omt_process_matches_command "${pid}" "${OMT_RECEIVER_COMMAND}"; then
        wait "${pid}" 2>/dev/null || true
        echo "OMT receiver failed to stay running" >&2
        return 1
    fi
    tmp="${PID_FILE}.tmp.$$"
    printf '%s %s\n' "${pid}" "${start_time}" > "${tmp}"
    chmod 600 "${tmp}"
    mv -f -- "${tmp}" "${PID_FILE}"
    echo "Started OMT receiver (pid ${pid})"
}

status_locked() {
    local record pid
    record="$(get_managed_record 2>/dev/null || true)"
    if [[ -z "${record}" ]]; then
        # The receiver is provably gone, so its last published status is a
        # leftover. Clearing it with the PID record lets the dashboard report
        # "stopped" instead of an unexplained "status stale". Only `start`
        # publishes a new one, and it holds this same lock.
        rm -f -- "${PID_FILE}" "${STATUS_FILE}"
        echo "stopped"
        return 3
    fi
    pid="${record%% *}"
    printf 'running:%s\n' "${pid}"
}

action="${1:-}"
exec 9>"${LOCK_FILE}"
flock 9
case "${action}" in
    start) start_locked ;;
    stop) stop_locked ;;
    restart)
        stop_locked >/dev/null 2>&1 || [[ "$?" -eq 3 ]]
        start_locked
        ;;
    status) status_locked ;;
    *)
        echo "Usage: $0 {start|stop|restart|status}" >&2
        exit 2
        ;;
esac
