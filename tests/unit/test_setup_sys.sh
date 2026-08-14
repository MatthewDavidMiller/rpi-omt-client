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

require 'free_boot_media' "setup-sys must release the boot media before setup-disk"
require 'copy-modloop' "setup-sys must move kernel modules into RAM before unmounting the boot media"
require 'apk add --no-cache sfdisk' "setup-sys must install sfdisk while the apk mirrors are still reachable"
require 'dev_on_disk' "setup-sys must share one disk/partition matcher for mount detection and unmount"
require 'rc-service -q modloop status' "setup-sys must run copy-modloop only when the modloop service is live"

if grep -Eq 'copy-modloop \|\| true' "${SETUP_SYS}"; then
    fail "copy-modloop failure must abort before the boot media is unmounted"
fi
if grep -Eq 'index\(\$1, d\)' "${SETUP_SYS}"; then
    fail "boot-media unmount must not use a raw prefix match; sdaa1 is not a partition of sda"
fi

# Ordering is the whole correctness story for this script, and every one of
# these was a real failure before it was a test:
#   - setup-disk exits 0 from paths that install nothing, so a completion
#     marker not gated on the new root filesystem is a false success;
#   - the boot media has to be released or setup-disk takes one of those paths;
#   - and the passwords have to be set before an apk OpenSSH install can
#     replace the sshd that accepts the factory image's empty root password,
#     or a run that stops in between leaves a board nobody can log in to.
while read -r problem; do
    [[ -n "${problem}" ]] && fail "${problem}"
done < <(awk '
    /^} \| chpasswd$/ { credentials = NR }
    /^install_local_prereqs$/ { prereqs = NR }
    /^free_boot_media$/ { freed = NR }
    /setup-disk -q -m sys/ { installed = NR }
    /^\[ -b "\$\{ROOT_PART\}" \] \|\|/ { verified = NR }
    /apk add --root \/mnt\/omt-newroot/ { intoroot = NR }
    /cp -a \/etc\/ssh\/ssh_host_/ { keys = NR }
    /^echo "\$\{SETUP_COMPLETE_MARKER\}"/ { marker = NR }
    END {
        if (!credentials || !prereqs || credentials > prereqs)
            print "passwords must be set before OpenSSH is installed"
        if (!freed || !installed || freed > installed)
            print "boot media must be freed before setup-disk runs"
        if (!verified || verified < installed)
            print "the root partition must be verified after setup-disk"
        if (!intoroot || intoroot < installed)
            print "network packages must be installed into the new root, after setup-disk"
        if (!keys || keys < intoroot)
            print "host keys must be copied after apk installs OpenSSH into the new root"
        if (!marker || marker < verified)
            print "the completion marker must come after the verification"
    }
' "${SETUP_SYS}")

# The firmware the appliance needs to rejoin Wi-Fi lives on a read-only
# modloop and belongs to no package, so it can only be installed with --root.
if grep -Eq '^apk add --no-cache[^|]*linux-firmware-brcm' "${SETUP_SYS}"; then
    fail "linux-firmware-brcm cannot be installed into the running factory image; use --root"
fi
require 'apk add --root /mnt/omt-newroot --no-cache wpa_supplicant iw linux-firmware-brcm' \
    "the new root must get the Wi-Fi firmware or a Wi-Fi-only board never comes back"

# The discovery rule that made setup-disk a silent no-op: alpine-conf skips any
# disk with a mounted partition, and a factory image boots with its own boot
# partition mounted. Exercise the real function against a synthetic
# /proc/mounts rather than trusting a grep.
mounts_probe="$(mktemp)"
helper="$(mktemp)"
trap 'rm -f -- "${mounts_probe}" "${helper}"' EXIT
sed -n '/^dev_on_disk()/,/^}/p; /^disk_has_mounted_part()/,/^}/p' "${SETUP_SYS}" \
    | sed "s#/proc/mounts#${mounts_probe}#g" > "${helper}"
[[ -s "${helper}" ]] || fail "disk_has_mounted_part is missing from setup-sys"
grep -q 'dev_on_disk' "${helper}" || fail "disk_has_mounted_part must call dev_on_disk"

# A factory Pi: the FAT boot partition of the install disk is mounted.
cat > "${mounts_probe}" <<'EOF'
tmpfs / tmpfs rw,relatime,mode=755 0 0
/dev/mmcblk0p1 /media/mmcblk0p1 vfat ro,relatime,errors=remount-ro 0 0
EOF
if sh -c ". '${helper}'; disk_has_mounted_part /dev/mmcblk0"; then
    :
else
    fail "disk_has_mounted_part must detect the mounted boot partition that blocks setup-disk"
fi

# The same disk once the boot media has been released.
cat > "${mounts_probe}" <<'EOF'
tmpfs / tmpfs rw,relatime,mode=755 0 0
EOF
if sh -c ". '${helper}'; disk_has_mounted_part /dev/mmcblk0"; then
    fail "disk_has_mounted_part must report a freed disk as available"
fi

# The new-root reachability check is a glob test, and a quoted glob silently
# never matches -- which made this check fire on every run regardless of what
# was actually installed. Exercise it against a real directory tree.
newroot_probe="$(mktemp -d)"
newroot_helper="$(mktemp)"
sed -n '/^newroot_has()/,/^}/p' "${SETUP_SYS}" \
    | sed "s#/mnt/omt-newroot#${newroot_probe}#g" > "${newroot_helper}"
[[ -s "${newroot_helper}" ]] || fail "newroot_has is missing from setup-sys"

mkdir -p "${newroot_probe}/lib/firmware/brcm" "${newroot_probe}/usr/sbin" \
    "${newroot_probe}/sbin"
touch "${newroot_probe}/usr/sbin/sshd"
if sh -c ". '${newroot_helper}'; newroot_has 'usr/sbin/sshd' 'sbin/sshd'"; then
    :
else
    fail "newroot_has must find a plain path that exists"
fi
if sh -c ". '${newroot_helper}'; newroot_has 'lib/firmware/brcm/brcmfmac*'"; then
    fail "newroot_has must not report a match in an empty firmware directory"
fi
touch "${newroot_probe}/lib/firmware/brcm/brcmfmac43455-sdio.bin.zst"
if sh -c ". '${newroot_helper}'; newroot_has 'lib/firmware/brcm/brcmfmac*'"; then
    :
else
    fail "newroot_has must expand the glob; a quoted pattern never matches"
fi
if sh -c ". '${newroot_helper}'; newroot_has 'usr/sbin/nothing-here'"; then
    fail "newroot_has must report a missing plain path as missing"
fi

# Alpine ships wpa_supplicant at sbin/, not usr/sbin/. Checking only the
# second one aborted a complete install on hardware, so both are asked about
# and this pins that either layout satisfies the check.
touch "${newroot_probe}/sbin/wpa_supplicant"
if sh -c ". '${newroot_helper}'; newroot_has 'sbin/wpa_supplicant' 'usr/sbin/wpa_supplicant'"; then
    :
else
    fail "newroot_has must accept wpa_supplicant at Alpine's sbin/ location"
fi
rm -f "${newroot_probe}/sbin/wpa_supplicant"
touch "${newroot_probe}/usr/sbin/wpa_supplicant"
if sh -c ". '${newroot_helper}'; newroot_has 'sbin/wpa_supplicant' 'usr/sbin/wpa_supplicant'"; then
    :
else
    fail "newroot_has must accept wpa_supplicant on a merged-/usr layout"
fi
rm -rf -- "${newroot_probe}" "${newroot_helper}"

# The paths the reachability check asks about are the ones Alpine's packages
# actually use; getting one wrong reads as a failed install.
require "newroot_has 'sbin/wpa_supplicant' 'usr/sbin/wpa_supplicant'" \
    "wpa_supplicant lives at sbin/ in Alpine's package, so that path must be checked"

# A different disk's partition must not block this one.
cat > "${mounts_probe}" <<'EOF'
/dev/sda1 /media/sda1 vfat ro,relatime 0 0
EOF
if sh -c ". '${helper}'; disk_has_mounted_part /dev/mmcblk0"; then
    fail "disk_has_mounted_part must not confuse another disk's partition for this one"
fi

# A name that merely starts with the disk name is a different disk: sdaa1 is
# a partition of sdaa, not of sda. Erasing on this answer makes it worth a test.
cat > "${mounts_probe}" <<'EOF'
/dev/sdaa1 /media/sdaa1 ext4 rw,relatime 0 0
EOF
if sh -c ". '${helper}'; disk_has_mounted_part /dev/sda"; then
    fail "disk_has_mounted_part must not treat sdaa1 as a partition of sda"
fi

# NVMe namespaces use the same p<index> rule.
cat > "${mounts_probe}" <<'EOF'
/dev/nvme0n1p2 / ext4 rw,relatime 0 0
EOF
if sh -c ". '${helper}'; disk_has_mounted_part /dev/nvme0n1"; then
    :
else
    fail "disk_has_mounted_part must detect a mounted NVMe partition"
fi

# free_boot_media must use the same matcher, not a prefix that would unmount
# another disk's partition.
grep -A80 '^free_boot_media()' "${SETUP_SYS}" | grep -q 'dev_on_disk' || \
    fail "free_boot_media must unmount with dev_on_disk, not a device-name prefix"

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
