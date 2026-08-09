#!/bin/bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
HELPER="${ROOT}/deploy/host/host-reboot.sh"
REQUEST_LIB="${ROOT}/deploy/lib/reboot-request.sh"

bash -n "${HELPER}"
bash -n "${REQUEST_LIB}"

require() {
    local text="$1" label="$2"
    grep -Fq -- "${text}" "${HELPER}" "${REQUEST_LIB}" || {
        echo "FAIL: ${label}" >&2
        exit 1
    }
}

require 'reboot-request.sh' "reboot validator must share tested request helpers"
require 'reboot_parse_request_body' "reboot validator must parse through the shared helper"
require 'reboot_evaluate_request' "reboot validator must evaluate through the shared helper"
require '[[ "${EUID}" -eq 0 ]]' "helper must require root"
require 'EXPECTED_UID="${OMT_UID:?OMT_UID is required}"' "expected UID must be installer supplied"
require 'EXPECTED_GID="${OMT_GID:?OMT_GID is required}"' "expected GID must be installer supplied"
require 'stat -c' "request paths must be inspected without following symlinks"
require 'stat -Lc' "opened descriptors must be inspected through /proc"
require 'MAX_REQUEST_BYTES=512' "request size must be bounded"
require 'MAX_AGE_SECONDS=30' "stale requests must be rejected"
require 'MAX_FUTURE_SECONDS=5' "future requests must be rejected"
require 'COOLDOWN_SECONDS=60' "reboot requests must have a cooldown"
require '[[ "${line_count}" -eq 4' "request schema must have exactly four fields"
require '[[ "${request_id}" =~ ^[0-9a-f]{32}$ ]]' "request IDs must be fixed nonces"
require 'replayed-request' "accepted request IDs must not replay"
require 'publish_result "${request_id}" accepted scheduled' "acceptance must be correlated"
require 'reboot_fixed_file_identity "${RESULT_FILE}"' \
    "result publication must validate empty and non-empty fixed regular files"
require 'logger --tag omt-client-reboot "accepted reboot request ${request_id}" || true' \
    "a syslog failure must not abort an accepted reboot"
require 'exec /sbin/reboot' "Alpine reboot command must be fixed"
if grep -q 'systemctl' "${HELPER}"; then
    echo "FAIL: Alpine reboot helper must not call systemd" >&2
    exit 1
fi

if grep -Eq '(^|[^[:alnum:]_])eval([^[:alnum:]_]|$)|(^|[^[:alnum:]_])(sh|bash) -c([^[:alnum:]_]|$)' \
        "${HELPER}" "${REQUEST_LIB}"; then
    echo "FAIL: reboot helper must not evaluate request-controlled commands" >&2
    exit 1
fi

echo "Host reboot bridge contract tests passed"
