#!/bin/bash
# Contract tests for the stock-Alpine bootstrap.
#
# This script runs before bash and sudo exist on the target, which is exactly
# what makes it easy to break: any bashism here fails on a clean image, and the
# failure looks like a broken deploy rather than a broken interpreter.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
BOOTSTRAP="${ROOT}/deploy/host/bootstrap.sh"
MANIFEST="${ROOT}/deploy/manifest-v3.txt"
DEPLOY="${ROOT}/scripts/deploy.sh"

failures=0
fail() {
    echo "FAIL: $1" >&2
    failures=$((failures + 1))
}
require() {
    grep -Eq -- "$1" "${BOOTSTRAP}" || fail "$2"
}

[[ -f "${BOOTSTRAP}" ]] || {
    echo "FAIL: deploy/host/bootstrap.sh is missing" >&2
    exit 1
}

# The interpreter contract: POSIX sh, because bash is what it installs.
[[ "$(head -n 1 "${BOOTSTRAP}")" == "#!/bin/sh" ]] || \
    fail "bootstrap must be a /bin/sh script; bash is not present when it runs"

# No gate skips: without a real POSIX shell this test proves nothing, and bash
# would happily accept the syntax that breaks the Pi. Repair with `make install`.
if command -v dash >/dev/null 2>&1; then
    dash -n "${BOOTSTRAP}" || fail "bootstrap is not valid POSIX sh"
elif command -v busybox >/dev/null 2>&1; then
    busybox sh -n "${BOOTSTRAP}" || fail "bootstrap is not valid busybox sh"
else
    echo "FAIL: neither dash nor busybox is installed; run make install" >&2
    exit 1
fi

# Bashisms that a bash-only reviewer would not notice, since bash accepts them.
for bashism in '\[\[' '=~' 'mapfile' 'local -a' '\$\{[A-Za-z_]+\[' 'declare '; do
    if grep -Eq -- "${bashism}" "${BOOTSTRAP}"; then
        fail "bootstrap uses a bashism (${bashism}) that busybox ash rejects"
    fi
done

require 'apk add .*bash' "bootstrap must install bash"
require 'apk add .*sudo' "bootstrap must install sudo"
require '/community' "bootstrap must enable the community repository for sudo"
require 'visudo -c' "a generated sudoers drop-in must be validated before publishing"
require '%wheel' "wheel must be granted escalation; Alpine grants it nothing"
require 'id -u.*=.*0|EUID' "bootstrap must refuse to run unprivileged"

# The series is read from the host, not hardcoded a second time.
grep -Eq 'SUPPORTED_ALPINE_SERIES=3\.24' "${BOOTSTRAP}" || \
    fail "bootstrap must pin the same Alpine series as the installer"

# A capsule that omits the bootstrap cannot install on a clean image at all.
grep -qxF 'deploy/host/bootstrap.sh' "${MANIFEST}" || \
    fail "deploy/host/bootstrap.sh must ship in the v3 manifest"

# The deploy path must actually invoke it, and must not assume sudo exists.
grep -q 'BOOTSTRAP_SCRIPT' "${DEPLOY}" || \
    fail "deploy.sh must bootstrap a host that has no bash or sudo"
if grep -Eq '"sudo install -d|&& sudo ' "${DEPLOY}"; then
    fail "deploy.sh must escalate through the detected method, not a literal sudo"
fi

if ((failures > 0)); then
    echo "${failures} bootstrap contract test(s) failed" >&2
    exit 1
fi

echo "Alpine bootstrap contract tests passed"
