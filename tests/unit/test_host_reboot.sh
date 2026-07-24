#!/bin/bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
HELPER="${ROOT}/deploy/host/host-reboot.sh"

bash -n "${HELPER}"

require() {
    local text="$1" label="$2"
    grep -Fq -- "${text}" "${HELPER}" || {
        echo "FAIL: ${label}" >&2
        exit 1
    }
}

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
require 'reject replayed-request' "accepted request IDs must not replay"
require 'publish_result "${request_id}" accepted scheduled' "acceptance must be correlated"
require 'exec /usr/bin/systemctl reboot --no-block' "reboot command must be fixed"

if grep -Eq 'eval|sh -c|bash -c' "${HELPER}"; then
    echo "FAIL: reboot helper must not evaluate request-controlled commands" >&2
    exit 1
fi

echo "Host reboot bridge contract tests passed"
