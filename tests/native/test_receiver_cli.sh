#!/bin/sh
# Copyright (c) 2026 Matthew David Miller
# SPDX-License-Identifier: MIT
#
# The receiver's CLI is a trust boundary: control-omt.sh and the Rust Web
# diagnostics service both build argument vectors for it, and every rejection
# has to be a named exit-2 usage failure rather than a partially applied
# command. Exit 3 is reserved for "the command ran and the answer was no", which
# is what lets the diagnostics service tell a broken receiver from a silent one.
set -eu

receiver="$1"
failures=0

fail() {
    echo "FAIL: $1" >&2
    failures=$((failures + 1))
}

# Assert an invocation exits with an expected status, without letting `set -e`
# abort on the non-zero ones being tested.
expect_status() {
    expected="$1"
    description="$2"
    shift 2
    actual=0
    "$@" >/dev/null 2>&1 || actual=$?
    [ "${actual}" -eq "${expected}" ] ||
        fail "${description}: expected exit ${expected}, got ${actual}"
}

version="$(${receiver} --version)"
[ -n "${version}" ] || fail "--version printed nothing"

discovery="$(${receiver} discover --wait-ms 0 --json)"
[ "${discovery}" = "[]" ] || fail "empty discovery is not an empty JSON array"

# Command dispatch.
expect_status 2 "no arguments" "${receiver}"
expect_status 2 "unknown command" "${receiver}" frobnicate
expect_status 2 "a positional argument is not an option" "${receiver}" discover extra

# Option parsing. Every one of these leaves the command unrun.
expect_status 2 "duplicate option" "${receiver}" discover --json --json
expect_status 2 "option without a value" "${receiver}" discover --wait-ms
expect_status 2 "option not valid for the command" "${receiver}" discover --target Camera --json
expect_status 2 "unknown option" "${receiver}" discover --colour red --json
expect_status 2 "missing required flag" "${receiver}" discover --wait-ms 0
expect_status 2 "numeric option below its range" "${receiver}" discover --wait-ms -1 --json
expect_status 2 "numeric option above its range" "${receiver}" discover --wait-ms 60001 --json
expect_status 2 "non-numeric numeric option" "${receiver}" discover --wait-ms 1e3 --json
expect_status 2 "trailing garbage in a numeric option" "${receiver}" discover --wait-ms 10x --json

# probe: shared target validation, then required options.
expect_status 2 "probe without a target" "${receiver}" probe --timeout-ms 1 --json
expect_status 2 "probe with an empty target" "${receiver}" probe --target "" --timeout-ms 1 --json
expect_status 2 "probe target with a path" \
    "${receiver}" probe --target 'omt://127.0.0.1:65000/path' --timeout-ms 1 --json
expect_status 2 "probe target with credentials" \
    "${receiver}" probe --target 'omt://user@127.0.0.1:65000' --timeout-ms 1 --json
expect_status 2 "probe target with port 0" \
    "${receiver}" probe --target 'omt://127.0.0.1:0' --timeout-ms 1 --json
expect_status 2 "probe source name with a control character" \
    "${receiver}" probe --target "$(printf 'Cam\tera')" --timeout-ms 1 --json

# play: required options and the connector allow-list.
expect_status 2 "play without a status file" "${receiver}" play --target Camera
expect_status 2 "play without a target" "${receiver}" play --status-file /tmp/omt-cli-test.json
expect_status 2 "play with an invalid connector" \
    "${receiver}" play --target Camera --connector invalid --status-file /tmp/omt-cli-test.json
expect_status 2 "play with a retry below its range" \
    "${receiver}" play --target Camera --retry-seconds 0 --status-file /tmp/omt-cli-test.json
expect_status 2 "play with a retry above its range" \
    "${receiver}" play --target Camera --retry-seconds 31 --status-file /tmp/omt-cli-test.json
expect_status 2 "play with a diagnostics-only option" \
    "${receiver}" play --target Camera --status-file /tmp/omt-cli-test.json --json

# A valid target that answers nothing is a reported negative, not a usage error.
probe_status=0
probe_output="$(${receiver} probe --target 'omt://127.0.0.1:1' --timeout-ms 200 --json)" ||
    probe_status=$?
[ "${probe_status}" -eq 3 ] || fail "an unreachable direct target did not exit 3"
case "${probe_output}" in
    '{"ok":false,"target":"omt://127.0.0.1:1",'*'"error":"'*'"}') ;;
    *) fail "unreachable probe output is not the documented JSON: ${probe_output}" ;;
esac

if [ "${failures}" -ne 0 ]; then
    echo "${failures} native receiver CLI contract(s) failed" >&2
    exit 1
fi

echo "native receiver CLI contracts passed"
