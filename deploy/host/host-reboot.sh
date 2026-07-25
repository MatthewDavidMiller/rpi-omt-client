#!/bin/bash
# Fixed, request-correlated host reboot bridge for the unprivileged Web GUI.

set -euo pipefail
export LC_ALL=C
umask 027

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
if [[ -f "${SCRIPT_DIR}/reboot-request.sh" ]]; then
    # Installed next to this script under /usr/local/libexec/omt-client.
    # shellcheck source=reboot-request.sh
    source "${SCRIPT_DIR}/reboot-request.sh"
elif [[ -f "${SCRIPT_DIR}/../lib/reboot-request.sh" ]]; then
    # Running from the nested deployment capsule.
    # shellcheck source=../lib/reboot-request.sh
    source "${SCRIPT_DIR}/../lib/reboot-request.sh"
else
    echo "reboot-request.sh is missing beside host-reboot.sh" >&2
    exit 1
fi

REQUEST_FILE="${OMT_REBOOT_REQUEST_FILE:-/var/lib/omt-client/host-actions/reboot.request}"
RESULT_FILE="${OMT_REBOOT_RESULT_FILE:-/var/lib/omt-client/host-actions/reboot.result}"
LOCK_FILE="${OMT_REBOOT_LOCK_FILE:-/run/lock/omt-client-reboot.lock}"
LAST_ACCEPTED_FILE="${OMT_REBOOT_LAST_ACCEPTED_FILE:-/var/lib/omt-client/host-actions/last-accepted}"
EXPECTED_UID="${OMT_UID:?OMT_UID is required}"
EXPECTED_GID="${OMT_GID:?OMT_GID is required}"
MAX_REQUEST_BYTES=512
MAX_AGE_SECONDS=30
MAX_FUTURE_SECONDS=5
COOLDOWN_SECONDS=60

[[ "${EUID}" -eq 0 ]] || {
    echo "host-reboot.sh must run as root" >&2
    exit 1
}
[[ "${EXPECTED_UID}" =~ ^[1-9][0-9]*$ && "${EXPECTED_GID}" =~ ^[1-9][0-9]*$ ]] || {
    echo "invalid expected OMT UID/GID" >&2
    exit 1
}

exec 9>"${LOCK_FILE}"
flock -n 9 || exit 0

publish_result() {
    local request_id="$1" status="$2" detail="$3" before after
    before="$(stat -c '%d:%i:%F:%u:%g:%a' -- "${RESULT_FILE}" 2>/dev/null || true)"
    [[ "${before}" == *":regular file:0:${EXPECTED_GID}:640" ]] || {
        echo "unsafe reboot result file" >&2
        return 1
    }
    printf 'version=1\nrequest_id=%s\nstatus=%s\ndetail=%s\n' \
        "${request_id}" "${status}" "${detail}" > "${RESULT_FILE}"
    chmod 0640 "${RESULT_FILE}"
    chown "root:${EXPECTED_GID}" "${RESULT_FILE}"
    sync -f "${RESULT_FILE}"
    after="$(stat -c '%d:%i:%F:%u:%g:%a' -- "${RESULT_FILE}" 2>/dev/null || true)"
    [[ "${before%%:regular file:*}" == "${after%%:regular file:*}" ]]
}

before="$(stat -c '%d:%i:%s:%F:%u:%g:%a' -- "${REQUEST_FILE}" 2>/dev/null || true)"
[[ "${before}" =~ ^[0-9]+:[0-9]+:([0-9]+):regular\ file:${EXPECTED_UID}:${EXPECTED_GID}:600$ ]] || {
    echo "unsafe reboot request file" >&2
    exit 1
}
size="${BASH_REMATCH[1]}"
(( size > 0 && size <= MAX_REQUEST_BYTES )) || exit 0

exec 3<"${REQUEST_FILE}"
opened="$(stat -Lc '%d:%i:%s:%F:%u:%g:%a' "/proc/self/fd/3")"
[[ "${opened}" == "${before}" ]] || {
    echo "reboot request changed while opening" >&2
    exit 1
}
request="$(head -c "$((MAX_REQUEST_BYTES + 1))" <&3)"
after="$(stat -c '%d:%i:%s:%F:%u:%g:%a' -- "${REQUEST_FILE}")"
opened_after="$(stat -Lc '%d:%i:%s:%F:%u:%g:%a' "/proc/self/fd/3")"
exec 3<&-
[[ "${before}" == "${after}" && "${before}" == "${opened_after}" ]] || {
    echo "reboot request changed while reading" >&2
    exit 1
}
(( ${#request} <= MAX_REQUEST_BYTES )) || exit 1

# Schema failures exit without a correlated result: request_id may be untrusted.
if ! reboot_parse_request_body >/dev/null; then
    echo "invalid reboot request body" >&2
    exit 1
fi

reject() {
    local detail="$1"
    publish_result "${request_id}" rejected "${detail}"
    : > "${REQUEST_FILE}"
    chown "${EXPECTED_UID}:${EXPECTED_GID}" "${REQUEST_FILE}"
    chmod 0600 "${REQUEST_FILE}"
    sync -f "${REQUEST_FILE}"
    exit 0
}

now="$(date +%s)"
last_accepted_raw=""
if [[ -f "${LAST_ACCEPTED_FILE}" && ! -L "${LAST_ACCEPTED_FILE}" ]]; then
    last_accepted_raw="$(head -c 128 -- "${LAST_ACCEPTED_FILE}" 2>/dev/null || true)"
fi
if ! reason="$(reboot_evaluate_request)"; then
    reject "${reason}"
fi

printf '%s %s\n' "${request_id}" "${now}" > "${LAST_ACCEPTED_FILE}"
chown root:root "${LAST_ACCEPTED_FILE}"
chmod 0600 "${LAST_ACCEPTED_FILE}"
sync -f "${LAST_ACCEPTED_FILE}"

: > "${REQUEST_FILE}"
chown "${EXPECTED_UID}:${EXPECTED_GID}" "${REQUEST_FILE}"
chmod 0600 "${REQUEST_FILE}"
sync -f "${REQUEST_FILE}"
publish_result "${request_id}" accepted scheduled
logger --tag omt-client-reboot "accepted reboot request ${request_id}"

sleep 5
exec /usr/bin/systemctl reboot --no-block
