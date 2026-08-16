#!/bin/sh
# shellcheck shell=sh
# Convert a factory Alpine Raspberry Pi image into persistent sys mode.
#
# This is the remote half of the deployer's Alpine setup action. It must run
# as root under busybox ash: a clean image has no bash, and setup-alpine's
# own password prompts cannot be driven over SSH. Secrets arrive on stdin,
# never on the command line.
#
# stdin, one line each:
#   hostname
#   root password
#   pi password
#   SSID as lowercase hex (empty = Ethernet only, or keep an existing Wi-Fi
#     association already on the image)
#   WPA PSK as 64 lowercase hex digits (empty = Ethernet only / keep existing)

set -eu
export LC_ALL=C
umask 022

SUPPORTED_ALPINE_SERIES=3.24
SETUP_COMPLETE_MARKER="=== Alpine sys install complete ==="

# Reputable US HTTPS Alpine mirrors. Keep this list identical in
# bootstrap.sh and install.sh; tests/unit/test_setup_sys.sh compares them.
# BEGIN US HTTPS APK MIRRORS
US_HTTPS_APK_MIRRORS="
https://mirrors.edge.kernel.org/alpine
https://mirrors.ocf.berkeley.edu/alpine
https://mirror.math.princeton.edu/pub/alpinelinux
"
# END US HTTPS APK MIRRORS

# The appliance's 5 GHz band policy. Keep this identical to
# HOST_WIFI_FREQ_LIST in deploy/lib/service-install.sh, which this script
# cannot source: it runs on a factory image that has no bash and no copy of
# deploy/lib. tests/unit/test_setup_sys.sh compares the two.
# BEGIN WIFI FREQ LIST
WIFI_FREQ_LIST="5180 5200 5220 5240 5260 5280 5300 5320 5500 5520 5540 5560 5580 5600 5620 5640 5660 5680 5700 5720 5745 5765 5785 5805 5825"
# END WIFI FREQ LIST

[ "$(id -u)" = 0 ] || {
    echo "ERROR: setup-sys.sh must run as root." >&2
    exit 1
}
[ "$(uname -m)" = aarch64 ] || {
    echo "ERROR: This appliance supports only Alpine Linux aarch64." >&2
    exit 1
}
[ -r /etc/alpine-release ] || {
    echo "ERROR: This appliance supports only Alpine Linux." >&2
    exit 1
}
ALPINE_RELEASE="$(cat /etc/alpine-release)"
case "${ALPINE_RELEASE}" in
    "${SUPPORTED_ALPINE_SERIES}".*) ;;
    *)
        echo "ERROR: Alpine Linux ${SUPPORTED_ALPINE_SERIES}.x is required; detected ${ALPINE_RELEASE}." >&2
        exit 1
        ;;
esac
PI_MODEL="$(tr -d '\000' < /proc/device-tree/model 2>/dev/null || true)"
case "${PI_MODEL}" in
    "Raspberry Pi"*) ;;
    *)
        echo "ERROR: Unsupported hardware; detected ${PI_MODEL:-unknown}." >&2
        exit 1
        ;;
esac

ROOT_FS_TYPE="$(awk '$2 == "/" { print $3; exit }' /proc/mounts)"
case "${ROOT_FS_TYPE}" in
    tmpfs|overlay|squashfs|ramfs|"") ;;
    *)
        echo "ERROR: this host is already installed in persistent sys mode (${ROOT_FS_TYPE} root)." >&2
        echo "Use Deploy for the appliance; Alpine setup is only for a factory diskless image." >&2
        exit 1
        ;;
esac

IFS= read -r HOSTNAME || {
    echo "ERROR: hostname was not provided on stdin." >&2
    exit 1
}
IFS= read -r ROOT_PASSWORD || {
    echo "ERROR: root password was not provided on stdin." >&2
    exit 1
}
IFS= read -r PI_PASSWORD || {
    echo "ERROR: pi password was not provided on stdin." >&2
    exit 1
}
IFS= read -r SSID_HEX || SSID_HEX=
IFS= read -r WIFI_PSK || WIFI_PSK=

case "${HOSTNAME}" in
    [A-Za-z0-9]|[A-Za-z0-9][-A-Za-z0-9]*[A-Za-z0-9])
        [ "${#HOSTNAME}" -le 63 ] || {
            echo "ERROR: hostname must be at most 63 characters." >&2
            exit 1
        }
        ;;
    *)
        echo "ERROR: hostname must be a single DNS label." >&2
        exit 1
        ;;
esac
[ -n "${ROOT_PASSWORD}" ] && [ -n "${PI_PASSWORD}" ] || {
    echo "ERROR: root and pi passwords must not be empty." >&2
    exit 1
}

if [ -n "${SSID_HEX}" ]; then
    case "${SSID_HEX}" in
        *[!0-9a-f]*)
            echo "ERROR: Wi-Fi SSID hex is invalid." >&2
            exit 1
            ;;
    esac
    [ "${#SSID_HEX}" -ge 2 ] && [ "${#SSID_HEX}" -le 64 ] || {
        echo "ERROR: Wi-Fi SSID hex is invalid." >&2
        exit 1
    }
    case "${WIFI_PSK}" in
        [0-9a-f][0-9a-f]*)
            [ "${#WIFI_PSK}" -eq 64 ] || {
                echo "ERROR: Wi-Fi PSK must be 64 hexadecimal digits." >&2
                exit 1
            }
            ;;
        *)
            echo "ERROR: Wi-Fi PSK must be 64 hexadecimal digits." >&2
            exit 1
            ;;
    esac
fi

echo "Configuring Alpine ${ALPINE_RELEASE} on ${PI_MODEL}."

find_boot_part() {
    _mnt=
    for _dir in /media/mmcblk* /media/nvme* /media/sd* /media/* /boot; do
        [ -d "${_dir}" ] || continue
        [ -f "${_dir}/cmdline.txt" ] || [ -f "${_dir}/config.txt" ] || continue
        _mnt="${_dir}"
        break
    done
    [ -n "${_mnt}" ] || return 1
    awk -v m="${_mnt}" '$2 == m { print $1; exit }' /proc/mounts
}

disk_from_part() {
    _part="${1#/dev/}"
    case "${_part}" in
        *[0-9]p[0-9]*)
            printf '%s\n' "/dev/${_part%p[0-9]*}"
            ;;
        *[0-9])
            printf '%s\n' "/dev/$(printf '%s\n' "${_part}" | sed 's/[0-9][0-9]*$//')"
            ;;
        *)
            return 1
            ;;
    esac
}

root_part_from_disk() {
    case "$1" in
        *[0-9]) printf '%s\n' "${1}p2" ;;
        *) printf '%s\n' "${1}2" ;;
    esac
}

# Whether device $2 is a partition of disk $1 (either argument may carry a
# leading /dev/). A partition is the disk name followed by an index,
# optionally separated by "p": mmcblk0p1 and sda1 belong to mmcblk0 and sda,
# but sdaa1 does not belong to sda. sysfs is consulted first because that is
# what alpine-conf's is_available_disk uses.
dev_on_disk() {
    _want="${1#/dev/}"
    _have="${2#/dev/}"
    [ "${_have}" = "${_want}" ] && return 0
    [ -e "/sys/block/${_want}/${_have}" ] && return 0
    _suffix="${_have#"${_want}"}"
    [ "${_suffix}" = "${_have}" ] && return 1
    case "${_suffix#p}" in
        '' | *[!0-9]*) return 1 ;;
        *) return 0 ;;
    esac
}

# Whether any partition of this disk is mounted -- the exact question
# alpine-conf's is_available_disk asks before it will partition anything.
disk_has_mounted_part() {
    _disk="${1#/dev/}"
    for _dev in $(awk '$1 ~ /^\/dev\// { sub(/^\/dev\//, "", $1); print $1 }' /proc/mounts); do
        dev_on_disk "${_disk}" "${_dev}" && return 0
    done
    return 1
}

# Release the boot media so setup-disk will partition the disk.
#
# alpine-conf's is_available_disk rejects any disk that has a mounted
# partition, and a factory image boots with its own boot partition mounted --
# find_boot_part above depends on exactly that. Left alone, setup-disk finds
# no available disk, takes its "No disks available" branch, and *exits 0*
# without installing anything, which set -e cannot see. setup-alpine's
# interactive path escapes this by offering the boot media; do the same thing
# without a prompt, because this script's stdin is at EOF.
free_boot_media() {
    disk_has_mounted_part "${INSTALL_DISK}" || return 0
    echo "Releasing the boot media so ${INSTALL_DISK} can be partitioned..."
    # The kernel modules live in a squashfs on the boot partition. They have
    # to be in RAM before the unmount or the install loses its modules
    # halfway through. copy-modloop exits 1 when the modloop service is not
    # running; that means the modules are already local, so skip it. Any
    # other failure must abort -- unmounting anyway would drop the live
    # modules and firmware the rest of this script still needs.
    if command -v copy-modloop >/dev/null 2>&1 &&
        command -v rc-service >/dev/null 2>&1 &&
        rc-service -q modloop status; then
        DO_UMOUNT=1 copy-modloop || {
            echo "ERROR: copy-modloop failed; leaving the boot media mounted." >&2
            return 1
        }
    fi
    # copy-modloop only releases the media backing /.modloop. Unmount whatever
    # else on this disk is still mounted, deepest path first. Match partitions
    # with the same rule as disk_has_mounted_part: a prefix test would treat
    # sdaa1 as a partition of sda.
    _disk="${INSTALL_DISK#/dev/}"
    for _pair in $(awk '$1 ~ /^\/dev\// { sub(/^\/dev\//, "", $1); print $1 "#" $2 }' /proc/mounts | sort -t'#' -k2 -r); do
        _dev="${_pair%%#*}"
        _mnt="${_pair#*#}"
        dev_on_disk "${_disk}" "${_dev}" || continue
        umount "${_mnt}" 2>/dev/null || umount -l "${_mnt}" 2>/dev/null || true
    done
    if disk_has_mounted_part "${INSTALL_DISK}"; then
        echo "ERROR: ${INSTALL_DISK} still has a mounted partition, so setup-disk would" >&2
        echo "exit without installing. Free it and run Alpine setup again." >&2
        return 1
    fi
    return 0
}

first_ethernet() {
    _iface=
    for _path in /sys/class/net/*; do
        [ -d "${_path}" ] || continue
        _name="${_path##*/}"
        case "${_name}" in
            lo|bonding_masters) continue ;;
            wlan*|wl*|wifi*) continue ;;
        esac
        [ -e "${_path}/wireless" ] && continue
        [ -d "${_path}/phy80211" ] && continue
        _iface="${_name}"
        break
    done
    printf '%s\n' "${_iface:-eth0}"
}

first_wifi() {
    for _path in /sys/class/net/*; do
        [ -d "${_path}" ] || continue
        _name="${_path##*/}"
        if [ -e "${_path}/wireless" ] || [ -d "${_path}/phy80211" ]; then
            printf '%s\n' "${_name}"
            return 0
        fi
    done
    printf '%s\n' wlan0
}

# Headless first-boot images delete /etc/wpa_supplicant.conf after association
# but leave the operator's file on the FAT boot partition.
find_existing_wpa() {
    if [ -f /etc/wpa_supplicant/wpa_supplicant.conf ]; then
        printf '%s\n' /etc/wpa_supplicant/wpa_supplicant.conf
        return 0
    fi
    for _dir in /media/mmcblk* /media/nvme* /media/sd* /media/*; do
        [ -f "${_dir}/wpa_supplicant.conf" ] || continue
        printf '%s\n' "${_dir}/wpa_supplicant.conf"
        return 0
    done
    return 1
}

install_wpa_config_from() {
    _src=$1
    [ -f "${_src}" ] || return 1
    apk add --no-cache wpa_supplicant iw >/dev/null 2>&1 || \
        apk add --no-cache wpa_supplicant || true
    install -d -m 0700 /etc/wpa_supplicant
    WPA_TMP="$(mktemp /etc/wpa_supplicant/.wpa_supplicant.conf.XXXXXX)"
    umask 077
    # The same globals `host_wpa_supplicant_config` writes: the regulatory
    # country, which defaults to US when the image carries none, and the 5 GHz
    # scan list. This path is the hand-written boot-partition
    # wpa_supplicant.conf, so it is the one most likely to name a 2.4 GHz
    # network, and the least likely to have thought about `country=` at all.
    # The `freq_list` strip is anchored for the reason given there: it is a
    # legal per-network key and only the global is band policy.
    awk -v freq_list="${WIFI_FREQ_LIST}" '
        /^[ \t]*country[ \t]*=/ {
            sub(/^[ \t]*country[ \t]*=[ \t]*/, "")
            if ($0 != "") { country = $0 }
            next
        }
        /^[ \t]*(ctrl_interface|ctrl_interface_group|update_config)[ \t]*=/ { next }
        /^freq_list[ \t]*=/ { next }
        { body[++lines] = $0 }
        END {
            print "ctrl_interface=/run/wpa_supplicant"
            print "ctrl_interface_group=wheel"
            print "update_config=1"
            printf "country=%s\n", (country != "" ? country : "US")
            printf "freq_list=%s\n", freq_list
            for (i = 1; i <= lines; i++) { print body[i] }
        }
    ' "${_src}" > "${WPA_TMP}"
    umask 022
    chown root:root "${WPA_TMP}"
    chmod 0600 "${WPA_TMP}"
    mv -f "${WPA_TMP}" /etc/wpa_supplicant/wpa_supplicant.conf
}

sync_clock() {
    echo "Setting the clock so HTTPS certificates verify..."
    if command -v ntpd >/dev/null 2>&1 && \
        ntpd -n -q -p time.nist.gov -p time.google.com -p 0.pool.ntp.org; then
        date -u
        return 0
    fi
    echo "ERROR: could not set the clock; HTTPS apk mirrors will fail on a 1970 clock." >&2
    return 1
}

install_local_prereqs() {
    echo "Installing OpenSSH, CA certificates, and disk tools from local media..."
    # The bundle, not the ca-certificates package: the bundle is what supplies
    # the trust store the HTTPS mirrors need, it is what the factory image's
    # local repository actually carries, and asking here for a package that
    # repository does not have leaves it recorded in an unsatisfiable
    # /etc/apk/world. ca-certificates itself follows once a mirror is pinned.
    apk add --no-cache ca-certificates-bundle || true
    apk add --no-cache \
        openssh \
        openssh-server \
        openssh-sftp-server \
        openssh-keygen \
        e2fsprogs \
        e2fsprogs-extra \
        wpa_supplicant \
        iw || true
}

preserve_ssh_host_keys() {
    mkdir -p /etc/ssh
    chmod 0755 /etc/ssh
    if [ -f /tmp/.ALHB/ssh_host_ed25519_key ] || [ -f /tmp/.ALHB/ssh_host_rsa_key ]; then
        echo "Keeping the SSH host keys already presented by this image..."
        cp -a /tmp/.ALHB/ssh_host_* /etc/ssh/ 2>/dev/null || true
    fi
    if command -v ssh-keygen >/dev/null 2>&1; then
        ssh-keygen -A
    fi
    chmod 0600 /etc/ssh/ssh_host_* 2>/dev/null || true
    chmod 0644 /etc/ssh/ssh_host_*.pub 2>/dev/null || true
}

enable_password_ssh() {
    mkdir -p /etc/ssh/sshd_config.d
    printf '%s\n' \
        'PasswordAuthentication yes' \
        'KbdInteractiveAuthentication yes' \
        'PermitRootLogin yes' \
        > /etc/ssh/sshd_config.d/99-omt-alpine-setup.conf
    chmod 0644 /etc/ssh/sshd_config.d/99-omt-alpine-setup.conf
    if [ -f /etc/ssh/sshd_config ] && \
        ! grep -q '^Include /etc/ssh/sshd_config.d' /etc/ssh/sshd_config; then
        printf '\nInclude /etc/ssh/sshd_config.d/*.conf\n' >> /etc/ssh/sshd_config
    fi
}

pin_us_https_apk_mirrors() {
    _series="${SUPPORTED_ALPINE_SERIES}"
    _tmp="$(mktemp)"
    _ok=
    for _base in ${US_HTTPS_APK_MIRRORS}; do
        echo "Trying US HTTPS apk mirror ${_base}..."
        printf '%s/v%s/main\n' "${_base}" "${_series}" > "${_tmp}"
        printf '%s/v%s/community\n' "${_base}" "${_series}" >> "${_tmp}"
        cp "${_tmp}" /etc/apk/repositories
        if apk update; then
            echo "Pinned apk repositories to ${_base} (HTTPS)."
            _ok=yes
            break
        fi
        echo "Mirror ${_base} did not serve an index; trying the next US HTTPS mirror."
    done
    rm -f -- "${_tmp}"
    [ "${_ok}" = yes ] || {
        echo "ERROR: no reputable US HTTPS apk mirror responded." >&2
        return 1
    }
    # Only now, with a full index reachable. The factory image's local
    # repository on the boot partition carries ca-certificates-bundle but not
    # ca-certificates, and asking for it there fails *after* recording it in
    # /etc/apk/world -- which leaves world unsatisfiable, so every later
    # transaction, including the ones that populate the new root, fails to
    # solve.
    apk add --no-cache ca-certificates
}

BOOT_PART="$(find_boot_part)" || {
    echo "ERROR: could not find the Alpine Raspberry Pi boot partition." >&2
    exit 1
}
INSTALL_DISK="$(disk_from_part "${BOOT_PART}")" || {
    echo "ERROR: could not determine the install disk for ${BOOT_PART}." >&2
    exit 1
}
[ -b "${INSTALL_DISK}" ] || {
    echo "ERROR: install disk ${INSTALL_DISK} is not a block device." >&2
    exit 1
}
ROOT_PART="$(root_part_from_disk "${INSTALL_DISK}")"
ETH_IFACE="$(first_ethernet)"
WIFI_IFACE="$(first_wifi)"
WIFI_ENABLED=
EXISTING_WPA=

if [ -n "${SSID_HEX}" ]; then
    WIFI_ENABLED=yes
elif EXISTING_WPA="$(find_existing_wpa)"; then
    WIFI_ENABLED=yes
fi

echo "Install disk is ${INSTALL_DISK} (boot ${BOOT_PART}, root ${ROOT_PART})."
if [ "${WIFI_ENABLED}" = yes ]; then
    echo "IPv4 DHCP on ${ETH_IFACE} and ${WIFI_IFACE}."
else
    echo "IPv4 DHCP on ${ETH_IFACE}."
fi

# Accounts and passwords come first, before anything can replace the sshd that
# is carrying this session.
#
# A factory image answers as root with an *empty* password, which the running
# sshd is configured to accept. Installing apk OpenSSH below can swap that
# sshd for one built from a stock config, where PermitEmptyPasswords is off.
# Setting the passwords first means the accounts are always ready before that
# can happen; leaving it until later opens a window where a run that stops in
# between -- for any reason -- leaves a board that no longer accepts the empty
# password and does not yet accept the new one, reachable only by power
# cycling it back to the factory overlay.
echo "Creating administrator account pi..."
if id pi >/dev/null 2>&1; then
    echo "User pi already exists; granting wheel."
    addgroup pi wheel || true
else
    adduser -D -g pi pi
    addgroup pi wheel || true
    addgroup pi audio || true
    addgroup pi input || true
    addgroup pi video || true
    addgroup pi netdev || true
fi

echo "Setting root and pi passwords..."
{
    printf 'root:%s\n' "${ROOT_PASSWORD}"
    printf 'pi:%s\n' "${PI_PASSWORD}"
} | chpasswd
ROOT_PASSWORD=
PI_PASSWORD=

install_local_prereqs
sync_clock
echo "Pinning apk repositories to reputable US HTTPS mirrors..."
pin_us_https_apk_mirrors

if ! command -v sshd >/dev/null 2>&1 && ! [ -x /usr/sbin/sshd ]; then
    echo "Installing OpenSSH from the HTTPS mirror..."
    apk add --no-cache openssh openssh-server openssh-sftp-server openssh-keygen
fi
preserve_ssh_host_keys
enable_password_ssh
# Do not restart sshd here: a headless overlay may already be listening with a
# deleted binary. Enable the apk OpenSSH service for the persistent root.
rc-update --quiet add sshd || true

if command -v setup-keymap >/dev/null 2>&1; then
    echo "Setting keymap to us..."
    setup-keymap us us || true
fi

echo "Setting hostname to ${HOSTNAME}..."
if command -v setup-hostname >/dev/null 2>&1; then
    setup-hostname "${HOSTNAME}"
else
    printf '%s\n' "${HOSTNAME}" > /etc/hostname
    hostname "${HOSTNAME}" || true
fi

echo "Writing IPv4 DHCP interfaces..."
{
    printf 'auto lo\niface lo inet loopback\n\n'
    printf 'auto %s\niface %s inet dhcp\n\thostname %s\n' \
        "${ETH_IFACE}" "${ETH_IFACE}" "${HOSTNAME}"
    if [ "${WIFI_ENABLED}" = yes ]; then
        printf '\nauto %s\niface %s inet dhcp\n\thostname %s\n' \
            "${WIFI_IFACE}" "${WIFI_IFACE}" "${HOSTNAME}"
    fi
} > /etc/network/interfaces
rc-update --quiet add networking boot || true

if [ -n "${SSID_HEX}" ]; then
    echo "Configuring Wi-Fi for first boot..."
    apk add --no-cache wpa_supplicant iw
    install -d -m 0700 /etc/wpa_supplicant
    WPA_TMP="$(mktemp /etc/wpa_supplicant/.wpa_supplicant.conf.XXXXXX)"
    umask 077
    {
        printf 'ctrl_interface=/run/wpa_supplicant\n'
        printf 'ctrl_interface_group=wheel\n'
        printf 'update_config=1\n'
        printf 'country=US\n'
        # 5 GHz only: the appliance does not support 2.4 GHz, so the SSID the
        # operator just entered is only joined on a band that can carry it.
        printf 'freq_list=%s\n' "${WIFI_FREQ_LIST}"
        printf 'network={\n'
        printf '\tssid=%s\n' "${SSID_HEX}"
        printf '\tpsk=%s\n' "${WIFI_PSK}"
        printf '\tkey_mgmt=WPA-PSK\n'
        printf '}\n'
    } > "${WPA_TMP}"
    umask 022
    chown root:root "${WPA_TMP}"
    chmod 0600 "${WPA_TMP}"
    mv -f "${WPA_TMP}" /etc/wpa_supplicant/wpa_supplicant.conf
    rc-update --quiet add wpa_supplicant boot || true
elif [ -n "${EXISTING_WPA}" ]; then
    echo "Keeping the Wi-Fi association already present on this image..."
    install_wpa_config_from "${EXISTING_WPA}"
    rc-update --quiet add wpa_supplicant boot || true
fi
WIFI_PSK=

if command -v setup-timezone >/dev/null 2>&1; then
    echo "Setting timezone to UTC..."
    setup-timezone UTC || true
fi
if command -v setup-proxy >/dev/null 2>&1; then
    setup-proxy -q none || true
fi
if command -v setup-ntp >/dev/null 2>&1; then
    echo "Configuring NTP..."
    setup-ntp busybox || true
fi

if command -v lbu >/dev/null 2>&1; then
    lbu add /home/pi 2>/dev/null || true
fi

command -v setup-disk >/dev/null 2>&1 || {
    echo "ERROR: setup-disk is missing; alpine-conf is required." >&2
    exit 1
}

# A factory image carries neither sfdisk nor the filesystem tools, and the
# local apk repository lives on the boot partition that is about to be
# released. Install them from the pinned HTTPS mirrors while that is still
# possible, and fail here rather than deep inside setup-disk.
echo "Installing partitioning and filesystem tools..."
apk add --no-cache sfdisk e2fsprogs e2fsprogs-extra dosfstools

free_boot_media

echo "Installing Alpine in persistent sys mode on ${INSTALL_DISK}..."
echo "This erases the disk and copies the running overlay onto a new root."
HEARTBEAT_PID=
(
    while sleep 15; do
        printf '%s\n' "Alpine sys install still running..."
        printf '%s\n' "Alpine sys install still running..." >&2
    done
) &
HEARTBEAT_PID=$!
stop_heartbeat() {
    if [ -n "${HEARTBEAT_PID}" ]; then
        kill "${HEARTBEAT_PID}" 2>/dev/null || true
        wait "${HEARTBEAT_PID}" 2>/dev/null || true
        HEARTBEAT_PID=
    fi
}
trap stop_heartbeat EXIT INT TERM

ERASE_DISKS="${INSTALL_DISK}"
export ERASE_DISKS
setup-disk -q -m sys "${INSTALL_DISK}"
stop_heartbeat
trap - EXIT INT TERM

# setup-disk returns 0 from several paths that install nothing at all, so the
# only trustworthy evidence is the root filesystem it was supposed to build.
# Everything below, including the completion marker the deployer keys on, is
# gated on finding it.
[ -b "${ROOT_PART}" ] || {
    echo "ERROR: setup-disk left no root partition at ${ROOT_PART}." >&2
    echo "The persistent sys install did not happen; this host is unchanged." >&2
    exit 1
}

echo "Mounting the new root to install network packages and copy host state..."
mkdir -p /mnt/omt-newroot
mount -t ext4 "${ROOT_PART}" /mnt/omt-newroot || {
    echo "ERROR: ${ROOT_PART} exists but is not a mountable ext4 root." >&2
    echo "The persistent sys install did not complete." >&2
    exit 1
}
[ -d /mnt/omt-newroot/etc ] && [ -d /mnt/omt-newroot/sbin ] || {
    umount /mnt/omt-newroot || true
    echo "ERROR: ${ROOT_PART} does not contain an Alpine root filesystem." >&2
    echo "The persistent sys install did not complete." >&2
    exit 1
}

# Whether the new root contains anything matching any of these globs.
#
# Each glob is deliberately unquoted: it is the shell that expands it, and
# `ls` handed a quoted pattern just looks for a file whose name contains an
# asterisk, so it always says no. Several candidates per question because
# `sbin` and `usr/sbin`, and `lib` and `usr/lib`, are the same place on a
# merged-/usr layout and different places otherwise; asking about both is
# steadier than tracking which Alpine release moved what.
newroot_has() {
    for _pattern in "$@"; do
        for _match in /mnt/omt-newroot/${_pattern}; do
            [ -e "${_match}" ] && return 0
        done
    done
    return 1
}

# Pin the live HTTPS repositories (and keys) before apk --root, so the
# transaction does not try the factory image's local media that we just
# unmounted. Do this before unpacking OpenSSH: a first-time package install
# can replace files that were copied in and not yet owned by apk.
if [ -d /etc/apk/keys ]; then
    mkdir -p /mnt/omt-newroot/etc/apk/keys
    cp -a /etc/apk/keys/. /mnt/omt-newroot/etc/apk/keys/ 2>/dev/null || true
fi
if [ -f /etc/apk/repositories ]; then
    mkdir -p /mnt/omt-newroot/etc/apk
    cp -a /etc/apk/repositories /mnt/omt-newroot/etc/apk/repositories
fi

# Everything the appliance needs in order to come back on the network, put
# into the new root rather than this one.
#
# setup-disk populates the new root from /etc/apk/world, and on a factory
# image that file lists almost nothing: the running system comes off a
# read-only modloop, so most of what it is using is not an installed package.
# The Broadcom Wi-Fi firmware under /lib/firmware belongs to no package at
# all, and /lib/firmware here cannot even be written to. So install into the
# target with --root, where the filesystem is real and writable. On a
# Wi-Fi-only board this is the difference between a reboot and a trip to the
# SD card reader.
echo "Installing the packages the persistent root needs to stay reachable..."
apk add --root /mnt/omt-newroot --no-cache \
    openssh openssh-server openssh-sftp-server openssh-keygen
if [ "${WIFI_ENABLED}" = yes ]; then
    echo "Adding Wi-Fi firmware and supplicant so ${WIFI_IFACE} comes back after reboot..."
    # wireless-regdb is what makes the country= written above mean anything:
    # the kernel reads /lib/firmware/regulatory.db, and without it the radio
    # stays in the world domain, where channels 149-165 do not exist and the
    # rest of 5 GHz cannot be initiated on. The board still associates -- on
    # 2.4 GHz, at a fraction of the throughput OMT video needs.
    apk add --root /mnt/omt-newroot --no-cache \
        wpa_supplicant iw linux-firmware-brcm wireless-regdb

    # Fall back to the firmware this board is running right now.
    #
    # The package route is preferred because it leaves apk owning the files,
    # but it has been observed to report success while installing nothing,
    # and the cost of getting this wrong on a Wi-Fi-only board is that the
    # board never comes back. The factory image's own /lib/firmware is the
    # strongest possible evidence -- it is driving this exact chip at this
    # moment -- so if the package did not land the files, copy them.
    if ! newroot_has 'lib/firmware/brcm/brcmfmac*' 'usr/lib/firmware/brcm/brcmfmac*' &&
        [ -d /lib/firmware/brcm ]; then
        echo "Package did not provide the firmware; copying it from the running image..."
        mkdir -p /mnt/omt-newroot/lib/firmware
        cp -a /lib/firmware/brcm /mnt/omt-newroot/lib/firmware/ 2>/dev/null || true
        if [ -d /lib/firmware/cypress ]; then
            cp -a /lib/firmware/cypress /mnt/omt-newroot/lib/firmware/ 2>/dev/null || true
        fi
    fi

    # The regulatory database gets the same treatment for the same reason: it
    # is a firmware-loader blob, apk can report success without placing it, and
    # its absence is silent -- the band simply is not there.
    if ! newroot_has 'lib/firmware/regulatory.db' 'usr/lib/firmware/regulatory.db'; then
        mkdir -p /mnt/omt-newroot/lib/firmware
        for blob in regulatory.db regulatory.db.p7s; do
            [ -f "/lib/firmware/${blob}" ] || continue
            cp -a "/lib/firmware/${blob}" /mnt/omt-newroot/lib/firmware/ \
                2>/dev/null || true
        done
    fi
fi

echo "Copying host keys and accounts onto the new root..."
mkdir -p /mnt/omt-newroot/etc/ssh
cp -a /etc/ssh/ssh_host_* /mnt/omt-newroot/etc/ssh/ 2>/dev/null || true
if [ -d /etc/ssh/sshd_config.d ]; then
    mkdir -p /mnt/omt-newroot/etc/ssh/sshd_config.d
    cp -a /etc/ssh/sshd_config.d/. /mnt/omt-newroot/etc/ssh/sshd_config.d/ \
        2>/dev/null || true
fi
if [ -f /etc/ssh/sshd_config ]; then
    cp -a /etc/ssh/sshd_config /mnt/omt-newroot/etc/ssh/sshd_config
fi
cp -a /etc/passwd /etc/shadow /etc/group /etc/hostname /mnt/omt-newroot/etc/
if [ -f /etc/hosts ]; then
    cp -a /etc/hosts /mnt/omt-newroot/etc/hosts
fi
if [ -f /etc/network/interfaces ]; then
    mkdir -p /mnt/omt-newroot/etc/network
    cp -a /etc/network/interfaces /mnt/omt-newroot/etc/network/interfaces
fi
if [ -f /etc/wpa_supplicant/wpa_supplicant.conf ]; then
    mkdir -p /mnt/omt-newroot/etc/wpa_supplicant
    cp -a /etc/wpa_supplicant/wpa_supplicant.conf \
        /mnt/omt-newroot/etc/wpa_supplicant/wpa_supplicant.conf
fi
if [ -d /etc/doas.d ]; then
    mkdir -p /mnt/omt-newroot/etc/doas.d
    cp -a /etc/doas.d/. /mnt/omt-newroot/etc/doas.d/ 2>/dev/null || true
fi

# Prove the new root can get back on the network before anyone reboots into
# it. The install itself has already happened by this point, so this is a
# report on what is on the disk, not an offer to undo it.
reachability_problem=
if ! newroot_has 'usr/sbin/sshd' 'sbin/sshd'; then
    reachability_problem="no sshd"
elif [ "${WIFI_ENABLED}" = yes ] &&
    ! newroot_has 'sbin/wpa_supplicant' 'usr/sbin/wpa_supplicant'; then
    reachability_problem="no wpa_supplicant"
elif [ "${WIFI_ENABLED}" = yes ] &&
    ! newroot_has 'lib/firmware/brcm/brcmfmac*' 'usr/lib/firmware/brcm/brcmfmac*'; then
    reachability_problem="no Broadcom Wi-Fi firmware"
fi
if [ -n "${reachability_problem}" ]; then
    umount /mnt/omt-newroot || true
    echo "ERROR: the persistent root on ${ROOT_PART} has ${reachability_problem}." >&2
    echo "Alpine is installed and this disk now boots it, but on Wi-Fi alone the board" >&2
    echo "may not come back. Do not count on this factory session for recovery: the" >&2
    echo "OpenSSH install above has already replaced the sshd serving it." >&2
    echo "Power cycle to boot the new root, and attach Ethernet if it does not appear." >&2
    exit 1
fi

umount /mnt/omt-newroot
rmdir /mnt/omt-newroot 2>/dev/null || true

echo "${SETUP_COMPLETE_MARKER}"
echo "Reboot to start the persistent sys install. SSH as pi, or as root with the passwords you set."
