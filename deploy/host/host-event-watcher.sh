#!/bin/bash
# Fixed-purpose inotify bridge used by the Alpine OpenRC services.

set -euo pipefail
export LC_ALL=C
umask 027

kind="${1:-}"
case "${kind}" in
    diagnostics)
        request_file="${OMT_DIAGNOSTICS_HOST_REQUEST_FILE:-/var/lib/omt-client/diagnostics/request}"
        action="${OMT_DIAGNOSTICS_ACTION:-/usr/local/libexec/omt-client/host-diagnostics.sh}"
        ;;
    reboot)
        request_file="${OMT_REBOOT_REQUEST_FILE:-/var/lib/omt-client/host-actions/reboot.request}"
        action="${OMT_REBOOT_ACTION:-/usr/local/libexec/omt-client/host-reboot.sh}"
        ;;
    *)
        echo "Usage: host-event-watcher.sh diagnostics|reboot" >&2
        exit 2
        ;;
esac

[[ "${EUID}" -eq 0 ]] || {
    echo "host-event-watcher.sh must run as root" >&2
    exit 1
}
[[ -f "${request_file}" && ! -L "${request_file}" && -x "${action}" ]] || {
    echo "unsafe ${kind} request or action path" >&2
    exit 1
}
command -v inotifywait >/dev/null 2>&1 || {
    echo "inotifywait is required" >&2
    exit 1
}

run_action() {
    if ! "${action}"; then
        logger -t "omt-client-${kind}" "fixed ${kind} action failed"
    fi
}

# Establish one persistent watch. Waiting for inotify's readiness message before
# checking the file closes the startup race: an event is either already queued
# or its non-empty request is visible to the check below.
watch_ready="$(mktemp /run/omt-client-inotify.XXXXXX)"
coproc OMT_WATCH {
    exec inotifywait --monitor --event close_write --format '%e' -- \
        "${request_file}" 2>"${watch_ready}"
}
watch_fd="${OMT_WATCH[0]}"
watch_pid="$!"
cleanup() {
    kill "${watch_pid}" 2>/dev/null || true
    wait "${watch_pid}" 2>/dev/null || true
    rm -f -- "${watch_ready}"
}
trap cleanup EXIT INT TERM HUP
watch_established=false
for _attempt in $(seq 1 100); do
    if grep -Fq 'Watches established.' "${watch_ready}"; then
        watch_established=true
        break
    fi
    kill -0 "${watch_pid}" 2>/dev/null || break
    sleep 0.01
done
rm -f -- "${watch_ready}"
[[ "${watch_established}" == true ]] || {
    echo "unable to establish ${kind} inotify watch" >&2
    exit 1
}

# Both actions independently validate age, inode, ownership, size, and schema
# before doing anything privileged.
if [[ -s "${request_file}" ]]; then
    run_action
fi

while IFS= read -r -u "${watch_fd}" _event; do
    run_action
done
logger -t "omt-client-${kind}" "persistent inotify watch exited"
exit 1
