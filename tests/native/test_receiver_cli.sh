#!/bin/sh
set -eu

receiver="$1"

version="$(${receiver} --version)"
test -n "${version}"

discovery="$(${receiver} discover --wait-ms 0 --json)"
test "${discovery}" = "[]"

if "${receiver}" discover --json --json >/dev/null 2>&1; then
    echo "duplicate CLI option was accepted" >&2
    exit 1
fi
if "${receiver}" play --target Camera --connector invalid --status-file /tmp/x >/dev/null 2>&1; then
    echo "invalid connector was accepted" >&2
    exit 1
fi
if "${receiver}" probe --target 'omt://127.0.0.1:65000/path' --timeout-ms 1 --json >/dev/null 2>&1; then
    echo "invalid direct target was accepted" >&2
    exit 1
fi

echo "native receiver CLI contracts passed"
