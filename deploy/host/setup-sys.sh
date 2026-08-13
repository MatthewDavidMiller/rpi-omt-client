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
    awk '
        BEGIN {
            print "ctrl_interface=/run/wpa_supplicant"
            print "ctrl_interface_group=wheel"
            print "update_config=1"
        }
        /^[ \t]*(ctrl_interface|ctrl_interface_group|update_config)[ \t]*=/ { next }
        { print }
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
    apk add --no-cache ca-certificates-bundle || true
    apk add --no-cache ca-certificates || true
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
    if ! [ -f /etc/ssl/certs/ca-certificates.crt ]; then
        echo "Installing CA certificates so apk can use HTTPS mirrors..."
        apk add --no-cache ca-certificates || true
    fi
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
WIFI_PSK=

if command -v lbu >/dev/null 2>&1; then
    lbu add /home/pi 2>/dev/null || true
fi

command -v setup-disk >/dev/null 2>&1 || {
    echo "ERROR: setup-disk is missing; alpine-conf is required." >&2
    exit 1
}

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

echo "Copying host keys and accounts onto the new root..."
if [ -b "${ROOT_PART}" ]; then
    mkdir -p /mnt/omt-newroot
    if mount -t ext4 "${ROOT_PART}" /mnt/omt-newroot; then
        mkdir -p /mnt/omt-newroot/etc/ssh
        cp -a /etc/ssh/ssh_host_* /mnt/omt-newroot/etc/ssh/ 2>/dev/null || true
        if [ -d /etc/ssh/sshd_config.d ]; then
            mkdir -p /mnt/omt-newroot/etc/ssh/sshd_config.d
            cp -a /etc/ssh/sshd_config.d/. /mnt/omt-newroot/etc/ssh/sshd_config.d/ \
                2>/dev/null || true
        fi
        [ -f /etc/ssh/sshd_config ] && \
            cp -a /etc/ssh/sshd_config /mnt/omt-newroot/etc/ssh/sshd_config
        cp -a /etc/passwd /etc/shadow /etc/group /etc/hostname \
            /mnt/omt-newroot/etc/
        [ -f /etc/hosts ] && cp -a /etc/hosts /mnt/omt-newroot/etc/hosts
        if [ -f /etc/apk/repositories ]; then
            mkdir -p /mnt/omt-newroot/etc/apk
            cp -a /etc/apk/repositories /mnt/omt-newroot/etc/apk/repositories
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
        umount /mnt/omt-newroot
    else
        echo "WARNING: could not mount ${ROOT_PART} to copy host keys; SSH may require a new known_hosts entry after reboot." >&2
    fi
    rmdir /mnt/omt-newroot 2>/dev/null || true
fi

echo "${SETUP_COMPLETE_MARKER}"
echo "Reboot to start the persistent sys install. SSH as pi, or as root with the passwords you set."
