#!/bin/bash
# Contract tests for the appliance's OpenRC service scripts.
#
# openrc-run executes these under /bin/sh, which is busybox ash on Alpine, and
# supervises them with supervise-daemon. Both facts have already cost a working
# install: the Avahi proxy shipped a start_post that could not create its socket
# and then left a respawning orphan behind, so the retry reported a collision
# with itself rather than the bind error underneath.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
OPENRC_DIR="${ROOT}/deploy/openrc"
AVAHI_PROXY="${OPENRC_DIR}/omt-client-avahi-proxy"

failures=0
fail() {
    echo "FAIL: $1" >&2
    failures=$((failures + 1))
}

[[ -d "${OPENRC_DIR}" ]] || {
    echo "FAIL: deploy/openrc is missing" >&2
    exit 1
}

shopt -s nullglob
SERVICES=("${OPENRC_DIR}"/*)
shopt -u nullglob
((${#SERVICES[@]} > 0)) || {
    echo "FAIL: no OpenRC service scripts found" >&2
    exit 1
}

# No gate skips: without a real POSIX shell this proves nothing, and bash would
# accept the very syntax that breaks on the Pi. Repair with `make install`.
if command -v dash >/dev/null 2>&1; then
    POSIX_SH=(dash -n)
elif command -v busybox >/dev/null 2>&1; then
    POSIX_SH=(busybox sh -n)
else
    echo "FAIL: neither dash nor busybox is installed; run make install" >&2
    exit 1
fi

for service in "${SERVICES[@]}"; do
    name="$(basename -- "${service}")"
    [[ "$(head -n 1 "${service}")" == "#!/sbin/openrc-run" ]] || \
        fail "${name} must be an openrc-run script"

    # openrc-run sources the body with /bin/sh, so the shebang above is not the
    # interpreter. Strip it and check what actually parses this file.
    body="$(tail -n +2 "${service}")"
    printf '%s\n' "${body}" | "${POSIX_SH[@]}" - 2>/dev/null || \
        fail "${name} is not valid POSIX sh; openrc-run has no bash to offer it"

    # Alpine's busybox is built with bash compatibility, so [[ ]] happens to
    # work today. It is not part of the shell these scripts are promised.
    if grep -Eq '\[\[' "${service}"; then
        fail "${name} uses [[ ]]; openrc-run service scripts must use POSIX test"
    fi
done

# The Avahi proxy is supervised with respawn_max=0, so a start_post that fails
# must not leave the supervisor running behind it.
grep -Eq 'respawn_max=0' "${AVAHI_PROXY}" || \
    fail "the Avahi proxy is expected to respawn without limit"
grep -Eq 'supervise-daemon "\$\{RC_SVCNAME\}" --stop' "${AVAHI_PROXY}" || \
    fail "a failed Avahi proxy start must stop its own supervisor, not orphan it"

# Compose accepts mem_limit and then ignores it when the kernel memory
# controller is off, which would run the appliance uncapped on a board sized
# around a 256 MiB container. The Pi firmware puts cgroup_disable=memory ahead
# of the installer's cgroup_enable=memory, so this is checked, not assumed.
APPLIANCE="${OPENRC_DIR}/omt-client"
grep -Eq 'until docker info' "${APPLIANCE}" || \
    fail "the appliance must wait for Docker API readiness, not only its OpenRC state"
grep -Eq 'docker_wait.*-ge 30' "${APPLIANCE}" || \
    fail "the Docker readiness wait must have a fixed upper bound"
grep -Eq 'cgroup\.controllers' "${APPLIANCE}" || \
    fail "the appliance must confirm the memory controller before starting uncapped"
grep -Eq '/proc/cgroups' "${APPLIANCE}" || \
    fail "the memory controller check must also cover a cgroup v1 host"

if ((failures > 0)); then
    echo "${failures} OpenRC service contract test(s) failed" >&2
    exit 1
fi

echo "OpenRC service contract tests passed"
