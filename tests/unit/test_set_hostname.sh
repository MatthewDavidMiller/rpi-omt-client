#!/bin/bash
# Contract tests for renaming an installed appliance.
#
# set-hostname.sh runs on the appliance's busybox ash, so this is the same kind
# of gate as tests/unit/test_setup_sys.sh: POSIX interpreter, no bashisms, and
# the deployer really uploads and invokes it. The two rewrites it performs are
# extracted from the script and run over fixtures, so the transformations are
# tested rather than merely grepped for.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
SET_HOSTNAME="${ROOT}/deploy/host/set-hostname.sh"
SETUP_SYS="${ROOT}/deploy/host/setup-sys.sh"
MANIFEST="${ROOT}/deploy/manifest-v3.txt"
OPS_RS="${ROOT}/crates/omt-deployer-core/src/ops.rs"
DEPLOYER_RS="${ROOT}/crates/rpi-omt-deployer/src/main.rs"
CLI_RS="${ROOT}/crates/rpi-omt-deploy/src/main.rs"

failures=0
fail() {
    echo "FAIL: $1" >&2
    failures=$((failures + 1))
}
require() {
    grep -Eq -- "$1" "${SET_HOSTNAME}" || fail "$2"
}

[[ -f "${SET_HOSTNAME}" ]] || {
    echo "FAIL: deploy/host/set-hostname.sh is missing" >&2
    exit 1
}

[[ "$(head -n 1 "${SET_HOSTNAME}")" == "#!/bin/sh" ]] || \
    fail "set-hostname must be a /bin/sh script; the appliance runs busybox ash"

if command -v dash >/dev/null 2>&1; then
    dash -n "${SET_HOSTNAME}" || fail "set-hostname is not valid POSIX sh"
elif command -v busybox >/dev/null 2>&1; then
    busybox sh -n "${SET_HOSTNAME}" || fail "set-hostname is not valid busybox sh"
else
    echo "FAIL: neither dash nor busybox is installed; run make install" >&2
    exit 1
fi

for bashism in '\[\[' '=~' 'mapfile' 'local -a' 'declare '; do
    if grep -Eq -- "${bashism}" "${SET_HOSTNAME}"; then
        fail "set-hostname uses a bashism (${bashism}) that busybox ash rejects"
    fi
done

require 'id -u.*=.*0' "set-hostname must refuse to run unprivileged"
require 'IFS= read -r NEW_HOSTNAME' \
    "the name must arrive on stdin, not in argv where a process listing shows it"

# The name has one definition across the product. A rename that accepted more
# than the factory installer does would leave a board with a name Alpine setup
# would have refused to write.
for rule in \
    '\[A-Za-z0-9\]\|\[A-Za-z0-9\]\[-A-Za-z0-9\]\*\[A-Za-z0-9\]' \
    '-le 63'
do
    require "${rule}" "set-hostname must apply the same DNS-label rule as setup-sys"
    grep -Eq -- "${rule}" "${SETUP_SYS}" || \
        fail "setup-sys no longer carries the hostname rule set-hostname is held to"
done

require '/etc/hostname' "the persistent hostname must be written"
require '^hostname "\$\{NEW_HOSTNAME\}"' "the running kernel hostname must be set"
require '/etc/hosts' "the loopback entry must follow the name"
require '/etc/network/interfaces' "the DHCP client identity must follow the name"
require 'rc-service avahi-daemon restart' "mDNS must publish the new name now, not at the next boot"

# The whole point of the action: Docker fills a host-network container's
# /etc/hostname when the container is created, so the Web GUI only shows a new
# name once a new container exists. The OpenRC service's stop is `compose
# down`, which is what makes its restart a recreation.
require 'rc-service omt-client restart' \
    "the appliance container must be recreated or the Web GUI keeps the old name"
require 'rc-service omt-client status' \
    "a deliberately stopped appliance must be left stopped"

# Renewing a lease would drop the SSH session this usually runs over.
grep -Eq -- 'ifdown|ifup|udhcpc|dhcpcd' "${SET_HOSTNAME}" && \
    fail "set-hostname must not bounce the interface it is running over"

# A `sh` without pipefail runs the right-hand side of a pipe in a subshell,
# where publish()'s refusals would exit that subshell and let the script carry
# on as though the file had been written.
grep -Eq -- '\| *publish ' "${SET_HOSTNAME}" && \
    fail "publish must not be the right-hand side of a pipe"

# The deployer's copy of the marker and the member path are what tell it the
# operation completed; a script that prints something else would be reported
# as an unfinished rename.
MARKER="$(sed -n 's/^COMPLETE_MARKER="\(.*\)"$/\1/p' "${SET_HOSTNAME}")"
[[ -n "${MARKER}" ]] || fail "set-hostname declares no completion marker"
grep -Fq "\"${MARKER}\"" "${OPS_RS}" || \
    fail "ops.rs does not wait for the marker set-hostname prints: ${MARKER}"
grep -Fq '"deploy/host/set-hostname.sh"' "${OPS_RS}" || \
    fail "ops.rs does not name the uploaded rename script"
grep -Fxq 'deploy/host/set-hostname.sh' "${MANIFEST}" || \
    fail "the rename script is not a capsule member, so it cannot be uploaded"

# Both front ends have to offer the action, or an operator's only route to it
# is a hand-run script on the Pi -- which is the gap this was added to close.
grep -Fq 'Hostname(HostnameArgs)' "${CLI_RS}" || \
    fail "the CLI has no hostname command"
grep -Fq 'can_set_hostname' "${DEPLOYER_RS}" || \
    fail "the desktop deployer does not gate a rename button"
grep -Fq 'Change hostname' "${DEPLOYER_RS}" || \
    fail "the desktop deployer's Manage view has no rename control"

# The two rewrites, run over fixtures. Both programs are read out of the
# script, so this tests what ships rather than a restatement of it.
WORK="$(mktemp -d)"
trap 'rm -rf -- "${WORK}"' EXIT

extract_awk() {
    awk -v want="$1" '
        /awk -v name=/ { block++; capture = (block == want); next }
        capture && /^    . \/etc\// { capture = 0 }
        capture { print }
    ' "${SET_HOSTNAME}"
}

HOSTS_PROGRAM="$(extract_awk 1)"
INTERFACES_PROGRAM="$(extract_awk 2)"
[[ -n "${HOSTS_PROGRAM}" && -n "${INTERFACES_PROGRAM}" ]] || {
    echo "FAIL: could not extract both rewrite programs from set-hostname.sh" >&2
    exit 1
}

printf '127.0.0.1\tlocalhost localhost.localdomain\n::1\t\tlocalhost\n127.0.1.1\told-name\n' \
    > "${WORK}/hosts"
awk -v name=new-name "${HOSTS_PROGRAM}" "${WORK}/hosts" > "${WORK}/hosts.out"
[[ "$(grep -c '^127\.0\.1\.1' "${WORK}/hosts.out")" == "1" ]] || \
    fail "repeated renames must not accumulate 127.0.1.1 entries in /etc/hosts"
grep -Fq '127.0.1.1	new-name' "${WORK}/hosts.out" || \
    fail "/etc/hosts must gain a loopback entry for the new name"
grep -Fq '127.0.0.1	localhost localhost.localdomain' "${WORK}/hosts.out" || \
    fail "/etc/hosts must keep the localhost line the distribution wrote"

{
    printf 'auto lo\niface lo inet loopback\n\n'
    printf 'auto eth0\niface eth0 inet dhcp\n\thostname old-name\n\n'
    printf 'auto wlan0\niface wlan0 inet dhcp\n\thostname old-name\n\tmetric 200\n'
} > "${WORK}/interfaces"
awk -v name=new-name "${INTERFACES_PROGRAM}" "${WORK}/interfaces" > "${WORK}/interfaces.out"
[[ "$(grep -c 'hostname new-name' "${WORK}/interfaces.out")" == "2" ]] || \
    fail "every interface's DHCP hostname option must be rewritten"
grep -q 'old-name' "${WORK}/interfaces.out" && \
    fail "a rewritten interfaces file must not keep the old name"
grep -Fq $'\tmetric 200' "${WORK}/interfaces.out" || \
    fail "the operator's other interface options must survive the rewrite"
[[ "$(wc -l < "${WORK}/interfaces")" == "$(wc -l < "${WORK}/interfaces.out")" ]] || \
    fail "the rewrite must not add or drop lines in /etc/network/interfaces"
grep -Fq $'\thostname new-name' "${WORK}/interfaces.out" || \
    fail "the rewrite must preserve each option's indentation"

((failures == 0)) || {
    echo "${failures} appliance rename contract test(s) failed" >&2
    exit 1
}
echo "Appliance rename contract tests passed"
