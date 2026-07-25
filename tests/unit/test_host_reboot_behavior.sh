#!/bin/bash
# Behavior tests for the shared reboot request validation helpers.

set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
# shellcheck source=../../deploy/lib/reboot-request.sh
source "${ROOT}/deploy/lib/reboot-request.sh"

MAX_AGE_SECONDS=30
MAX_FUTURE_SECONDS=5
COOLDOWN_SECONDS=60

expect_parse_fail() {
    local label="$1"
    request="$2"
    if reboot_parse_request_body >/dev/null 2>&1; then
        echo "FAIL: ${label} was accepted by the parser" >&2
        exit 1
    fi
}

expect_reject() {
    local label="$1" expected="$2"
    local reason
    reason="$(reboot_evaluate_request)" && {
        echo "FAIL: ${label} was accepted" >&2
        exit 1
    }
    [[ "${reason}" == "${expected}" ]] || {
        echo "FAIL: ${label} expected ${expected}, got ${reason}" >&2
        exit 1
    }
}

now=1700000000
request=$'version=1\naction=reboot\nrequest_id=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nrequested_at_epoch=1700000000'
request_id=""
reboot_parse_request_body >/dev/null
[[ "${request_id}" == "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" ]]
last_accepted_raw=""
reboot_evaluate_request

# Zero-padded epochs must compare as base 10 (same trap as host diagnostics).
# The timestamp grammar rejects a leading zero so the value cannot be read as
# octal by `(( ... ))`.
request=$'version=1\naction=reboot\nrequest_id=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\nrequested_at_epoch=0000001700000000'
reboot_parse_request_body >/dev/null
expect_reject "leading-zero epoch" "invalid-timestamp"

expect_parse_fail "extra field" \
    $'version=1\naction=reboot\nrequest_id=cccccccccccccccccccccccccccccccc\nrequested_at_epoch=1700000000\nextra=1'
expect_parse_fail "duplicate key" \
    $'version=1\naction=reboot\nrequest_id=dddddddddddddddddddddddddddddddd\nversion=1'
expect_parse_fail "short nonce" \
    $'version=1\naction=reboot\nrequest_id=abc\nrequested_at_epoch=1700000000'
expect_parse_fail "wrong line count" \
    $'version=1\naction=reboot\nrequest_id=eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee'

request=$'version=2\naction=reboot\nrequest_id=ffffffffffffffffffffffffffffffff\nrequested_at_epoch=1700000000'
reboot_parse_request_body >/dev/null
expect_reject "wrong version" "invalid-request"

request=$'version=1\naction=halt\nrequest_id=11111111111111111111111111111111\nrequested_at_epoch=1700000000'
reboot_parse_request_body >/dev/null
expect_reject "wrong action" "invalid-request"

request=$'version=1\naction=reboot\nrequest_id=22222222222222222222222222222222\nrequested_at_epoch=1'
reboot_parse_request_body >/dev/null
expect_reject "stale request" "stale-request"

request=$'version=1\naction=reboot\nrequest_id=33333333333333333333333333333333\nrequested_at_epoch=1700000600'
reboot_parse_request_body >/dev/null
expect_reject "future request" "future-request"

request=$'version=1\naction=reboot\nrequest_id=44444444444444444444444444444444\nrequested_at_epoch=1700000000'
reboot_parse_request_body >/dev/null
last_accepted_raw="44444444444444444444444444444444 1699999990"
expect_reject "replayed request" "replayed-request"

last_accepted_raw="55555555555555555555555555555555 1699999970"
expect_reject "cooldown active" "cooldown-active"

last_accepted_raw="55555555555555555555555555555555 1699999900"
reboot_evaluate_request

echo "Host reboot request validation tests passed"
