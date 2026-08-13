#!/bin/bash
# Contract tests for factory-Alpine sys-mode setup.
#
# setup-sys.sh runs on busybox ash before bash exists, so this gate is the
# same kind of check as tests/unit/test_bootstrap.sh: POSIX interpreter,
# no bashisms, and the deployer actually uploads and invokes it.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
SETUP_SYS="${ROOT}/deploy/host/setup-sys.sh"
BOOTSTRAP="${ROOT}/deploy/host/bootstrap.sh"
INSTALL="${ROOT}/deploy/host/install.sh"
MANIFEST="${ROOT}/deploy/manifest-v3.txt"
OPS_RS="${ROOT}/crates/omt-deployer-core/src/ops.rs"

failures=0
fail() {
    echo "FAIL: $1" >&2
    failures=$((failures + 1))
}
require() {
    grep -Eq -- "$1" "${SETUP_SYS}" || fail "$2"
}

[[ -f "${SETUP_SYS}" ]] || {
    echo "FAIL: deploy/host/setup-sys.sh is missing" >&2
    exit 1
}

[[ "$(head -n 1 "${SETUP_SYS}")" == "#!/bin/sh" ]] || \
    fail "setup-sys must be a /bin/sh script; bash is not present when it runs"

if command -v dash >/dev/null 2>&1; then
    dash -n "${SETUP_SYS}" || fail "setup-sys is not valid POSIX sh"
elif command -v busybox >/dev/null 2>&1; then
    busybox sh -n "${SETUP_SYS}" || fail "setup-sys is not valid busybox sh"
else
    echo "FAIL: neither dash nor busybox is installed; run make install" >&2
    exit 1
fi

for bashism in '\[\[' '=~' 'mapfile' 'local -a' '\$\{[A-Za-z_]+\[' 'declare '; do
    if grep -Eq -- "${bashism}" "${SETUP_SYS}"; then
        fail "setup-sys uses a bashism (${bashism}) that busybox ash rejects"
    fi
done

require 'id -u.*=.*0' "setup-sys must refuse to run unprivileged"
require 'SUPPORTED_ALPINE_SERIES=3\.24' "setup-sys must pin the same Alpine series as the installer"
require 'setup-disk -q -m sys' "setup-sys must install persistent sys mode"
require 'ERASE_DISKS=' "setup-sys must suppress setup-disk's erase prompt"
require 'adduser|setup-user' "setup-sys must create the pi administrator"
require 'chpasswd' "setup-sys must set root and pi passwords from stdin"
require 'iface .* inet dhcp' "setup-sys must use IPv4 DHCP"
require 'ctrl_interface=/run/wpa_supplicant' "Wi-Fi config must match the installer control socket"
require 'BEGIN US HTTPS APK MIRRORS' "setup-sys must pin US HTTPS apk mirrors"
require 'mirrors\.edge\.kernel\.org' "kernel.org is a required US HTTPS mirror"
require 'mirrors\.ocf\.berkeley\.edu' "Berkeley OCF is a required US HTTPS mirror"
require 'mirror\.math\.princeton\.edu' "Princeton is a required US HTTPS mirror"
require 'https://' "apk mirrors must be HTTPS"
if grep -Eq '^[^#]*http://' "${SETUP_SYS}"; then
    fail "setup-sys must not use HTTP apk mirrors"
fi
require '=== Alpine sys install complete ===' "setup-sys must print a completion marker"
require 'Alpine sys install still running' "setup-sys must heartbeat during setup-disk"
require 'ntpd -n -q' "setup-sys must set the clock before HTTPS apk fetches"
require 'openssh-sftp-server' "setup-sys must install apk OpenSSH, not trust a headless overlay sshd"
require 'PermitRootLogin yes' "setup-sys must allow password SSH as root until the appliance installer hardens it"
grep -Fq '${_dir}/wpa_supplicant.conf' "${SETUP_SYS}" || \
    fail "setup-sys must keep a boot-partition Wi-Fi association when no SSID is supplied"
if grep -q 'rc-service --quiet sshd status' "${SETUP_SYS}"; then
    fail "setup-sys must not skip OpenSSH install because a headless overlay sshd is already listening"
fi

grep -qxF 'deploy/host/setup-sys.sh' "${MANIFEST}" || \
    fail "deploy/host/setup-sys.sh must ship in the v3 manifest"

grep -Eq 'setup-sys\.sh|alpine_setup' "${OPS_RS}" || \
    fail "the native deployer must invoke setup-sys.sh"

# The three host scripts that fetch packages must name the same US HTTPS
# allowlist. A drift here is how a bootstrap and a sys install would silently
# use different mirrors.
extract_mirrors() {
    awk '
        /^# BEGIN US HTTPS APK MIRRORS$/ { grab = 1; next }
        /^# END US HTTPS APK MIRRORS$/ { grab = 0 }
        grab && NF { print }
    ' "$1"
}
setup_mirrors="$(extract_mirrors "${SETUP_SYS}")"
[[ -n "${setup_mirrors}" ]] || fail "setup-sys mirror allowlist is empty"
for host_script in "${BOOTSTRAP}" "${INSTALL}"; do
    [[ -f "${host_script}" ]] || {
        fail "missing ${host_script}"
        continue
    }
    other="$(extract_mirrors "${host_script}")"
    [[ "${other}" == "${setup_mirrors}" ]] || \
        fail "$(basename "${host_script}") US HTTPS apk mirror list does not match setup-sys.sh"
    if grep -Eq '^[^#]*http://' "${host_script}"; then
        fail "$(basename "${host_script}") must not use HTTP apk mirrors"
    fi
done

if ((failures > 0)); then
    echo "${failures} Alpine sys-setup contract test(s) failed" >&2
    exit 1
fi

echo "Alpine sys-setup contract tests passed"
