#!/bin/bash
# Raspberry Pi Alpine Linux appliance installer.
# Usage: sudo /absolute/path/to/omt-client/deploy/host/install.sh \
#            [--hdmi-video auto|HDMI-A-[12]:WIDTHxHEIGHT@HZ] \
#            [--max-video auto|WIDTHxHEIGHT@FPS[,...]]

set -euo pipefail
export LC_ALL=C
umask 022

# The one Alpine series this appliance is validated against. Package names move
# between releases -- rfkill folded into util-linux-misc in 3.24 -- so the
# series is pinned rather than ranged, and the apk repository URLs below are
# derived from it instead of being spelled out a second time.
SUPPORTED_ALPINE_SERIES=3.24

usage() {
    cat <<'EOF'
Usage: install.sh [--hdmi-video MODE] [--max-video CEILING]

MODE is "auto" or a KMS connector and mode such as:
  HDMI-A-1:1920x1080@60
  HDMI-A-2:1280x720@60

CEILING is "auto" for this board's default, or one or more decode limits:
  1280x720@60
  1920x1080@30,1280x720@60

The host must run Alpine Linux 3.24 aarch64 in persistent sys mode on a
Raspberry Pi 5, Raspberry Pi 4 Model B, Raspberry Pi 3, or Raspberry Pi
Zero 2 W. With no option, a saved choice is preserved; first installs use auto.

Run deploy/host/bootstrap.sh as root first on a stock Alpine image: this
script needs bash and sudo, and Alpine ships neither.

Raising the ceiling above the board default is allowed but not validated: a
board that cannot decode the format will drop frames rather than refuse them.
EOF
}

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
INSTALL_DIR="$(cd -- "${SCRIPT_DIR}/../.." && pwd -P)"
LIB_DIR="${INSTALL_DIR}/deploy/lib"
# shellcheck source=deploy/lib/host-validation.sh
source "${LIB_DIR}/host-validation.sh"
# shellcheck source=deploy/lib/board-profile.sh
source "${LIB_DIR}/board-profile.sh"
# shellcheck source=deploy/lib/hdmi-config.sh
source "${LIB_DIR}/hdmi-config.sh"
# shellcheck source=deploy/lib/publication.sh
source "${LIB_DIR}/publication.sh"
# shellcheck source=deploy/lib/service-install.sh
source "${LIB_DIR}/service-install.sh"

HDMI_VIDEO_EXPLICIT=false
HDMI_VIDEO_REQUEST=""
MAX_VIDEO_EXPLICIT=false
MAX_VIDEO_REQUEST=""
while (($#)); do
    case "$1" in
        --max-video)
            [[ $# -ge 2 && "${MAX_VIDEO_EXPLICIT}" == "false" ]] || {
                echo "ERROR: --max-video requires one value and may be used once." >&2
                exit 2
            }
            MAX_VIDEO_EXPLICIT=true
            MAX_VIDEO_REQUEST="$2"
            shift 2
            ;;
        --max-video=*)
            [[ "${MAX_VIDEO_EXPLICIT}" == "false" ]] || {
                echo "ERROR: --max-video may only be specified once." >&2
                exit 2
            }
            MAX_VIDEO_EXPLICIT=true
            MAX_VIDEO_REQUEST="${1#*=}"
            shift
            ;;
        --hdmi-video)
            [[ $# -ge 2 && "${HDMI_VIDEO_EXPLICIT}" == "false" ]] || {
                echo "ERROR: --hdmi-video requires one value and may be used once." >&2
                exit 2
            }
            HDMI_VIDEO_EXPLICIT=true
            HDMI_VIDEO_REQUEST="$2"
            shift 2
            ;;
        --hdmi-video=*)
            [[ "${HDMI_VIDEO_EXPLICIT}" == "false" ]] || {
                echo "ERROR: --hdmi-video may only be specified once." >&2
                exit 2
            }
            HDMI_VIDEO_EXPLICIT=true
            HDMI_VIDEO_REQUEST="${1#*=}"
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "ERROR: Unknown installer argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done
if [[ "${HDMI_VIDEO_EXPLICIT}" == "true" ]] && \
   ! host_validate_hdmi_video_mode "${HDMI_VIDEO_REQUEST}"; then
    echo "ERROR: Invalid --hdmi-video mode: ${HDMI_VIDEO_REQUEST}" >&2
    exit 2
fi
if [[ "${MAX_VIDEO_EXPLICIT}" == "true" && "${MAX_VIDEO_REQUEST}" != auto ]] && \
   ! host_validate_video_ceiling "${MAX_VIDEO_REQUEST}"; then
    echo "ERROR: Invalid --max-video ceiling: ${MAX_VIDEO_REQUEST}" >&2
    echo "Expected auto, or WIDTHxHEIGHT@FPS values within 1920x1080@60." >&2
    exit 2
fi
host_validate_safe_absolute_path "${INSTALL_DIR}" || {
    echo "ERROR: Invalid install directory: ${INSTALL_DIR}" >&2
    exit 1
}

TARBALL="${INSTALL_DIR}/omt-client-arm64.tar.gz"
COMPOSE_FILE="${INSTALL_DIR}/deploy/compose.yml"
COMPOSE_ENV_FILE="${INSTALL_DIR}/deploy/.env"
HOST_DIAGNOSTICS_SCRIPT="${INSTALL_DIR}/deploy/host/host-diagnostics.sh"
HOST_EVENT_WATCHER_SCRIPT="${INSTALL_DIR}/deploy/host/host-event-watcher.sh"
HOST_REBOOT_SCRIPT="${INSTALL_DIR}/deploy/host/host-reboot.sh"
HOST_REBOOT_REQUEST_LIB="${INSTALL_DIR}/deploy/lib/reboot-request.sh"
OPENRC_SOURCE_DIR="${INSTALL_DIR}/deploy/openrc"
PROJECT_LICENSE="${INSTALL_DIR}/LICENSE"
THIRD_PARTY_NOTICES="${INSTALL_DIR}/THIRD_PARTY_NOTICES.txt"
THIRD_PARTY_SOURCE="${INSTALL_DIR}/THIRD_PARTY_SOURCE.md"
DEPLOY_TRANSACTION_SCRIPT="${INSTALL_DIR}/deploy/transaction.sh"
DEPLOY_ARTIFACT_MANIFEST="${INSTALL_DIR}/deploy/manifest-v3.txt"

HOST_COMPONENT_DIR="/usr/local/libexec/omt-client"
HOST_DIAGNOSTICS_INSTALLED_SCRIPT="${HOST_COMPONENT_DIR}/host-diagnostics.sh"
HOST_EVENT_WATCHER_INSTALLED_SCRIPT="${HOST_COMPONENT_DIR}/host-event-watcher.sh"
HOST_REBOOT_INSTALLED_SCRIPT="${HOST_COMPONENT_DIR}/host-reboot.sh"
HOST_REBOOT_REQUEST_LIB_INSTALLED="${HOST_COMPONENT_DIR}/reboot-request.sh"
DEPLOY_RECOVERY_HELPER="${HOST_COMPONENT_DIR}/recover-deployment.sh"
DEPLOY_RECOVERY_MANIFEST="${HOST_COMPONENT_DIR}/manifest-v3.txt"
HOST_STATE_DIR="/var/lib/omt-client"
AVAHI_STATE_DIR="${HOST_STATE_DIR}/avahi"
HOST_DIAGNOSTICS_STATE_DIR="${HOST_STATE_DIR}/diagnostics"
HOST_ACTION_STATE_DIR="${HOST_STATE_DIR}/host-actions"
HOST_DIAGNOSTICS_REQUEST_FILE="${HOST_DIAGNOSTICS_STATE_DIR}/request"
HOST_DIAGNOSTICS_REPORT_FILE="${HOST_DIAGNOSTICS_STATE_DIR}/host-report.txt"
HOST_REBOOT_REQUEST_FILE="${HOST_ACTION_STATE_DIR}/reboot.request"
HOST_REBOOT_RESULT_FILE="${HOST_ACTION_STATE_DIR}/reboot.result"
AVAHI_PROXY_SOCKET="${AVAHI_STATE_DIR}/system-bus"
INSTALLER_CONFIG_DIR="/etc/omt-client"
INSTALLER_CONFIG_FILE="${INSTALLER_CONFIG_DIR}/installer.conf"
STABLE_VOLUME="omt-config-v3"
OPENRC_SERVICES=(
    omt-client
    omt-client-avahi-proxy
    omt-client-host-diagnostics
    omt-client-reboot
)

echo "=== Raspberry Pi Alpine OMT Client Installer ==="

# Preflight is deliberately complete before the first host mutation.
[[ "${EUID}" -eq 0 ]] || {
    echo "ERROR: Run this installer as root (sudo, doas, or su -c)." >&2
    echo "Stock Alpine has no sudo; deploy/host/bootstrap.sh installs it." >&2
    exit 1
}
command -v sshd >/dev/null 2>&1 || {
    echo "ERROR: OpenSSH server is required so hardening can be validated before reload." >&2
    exit 1
}
[[ "$(uname -m)" == "aarch64" ]] || {
    echo "ERROR: This appliance supports only Alpine Linux aarch64." >&2
    exit 1
}
[[ -r /etc/os-release && -r /etc/alpine-release ]] || {
    echo "ERROR: This appliance supports only Alpine Linux." >&2
    exit 1
}
# shellcheck source=/etc/os-release
source /etc/os-release
[[ "${ID:-}" == "alpine" && "$(</etc/alpine-release)" == "${SUPPORTED_ALPINE_SERIES}".* ]] || {
    echo "ERROR: Alpine Linux ${SUPPORTED_ALPINE_SERIES}.x is required; detected ${PRETTY_NAME:-unknown}." >&2
    exit 1
}
PI_MODEL="$(tr -d '\000' < /proc/device-tree/model 2>/dev/null || true)"
BOARD_PROFILE="$(host_board_profile "${PI_MODEL}")" || {
    echo "ERROR: Unsupported hardware; detected ${PI_MODEL:-unknown}." >&2
    echo "This appliance supports:" >&2
    host_supported_boards | sed 's/^/  - /' >&2
    exit 1
}
BOARD_ID="$(sed -n 's/^BOARD_ID=//p' <<< "${BOARD_PROFILE}")"
BOARD_LABEL="$(sed -n 's/^BOARD_LABEL=//p' <<< "${BOARD_PROFILE}")"
BOARD_HDMI_CONNECTORS="$(sed -n 's/^HDMI_CONNECTORS=//p' <<< "${BOARD_PROFILE}")"
BOARD_VIDEO_CEILING="$(sed -n 's/^VIDEO_CEILING=//p' <<< "${BOARD_PROFILE}")"
[[ -n "${BOARD_ID}" && -n "${BOARD_LABEL}" && -n "${BOARD_VIDEO_CEILING}" && \
   "${BOARD_HDMI_CONNECTORS}" =~ ^[12]$ ]] || {
    echo "ERROR: Board profile for ${PI_MODEL} is incomplete." >&2
    exit 1
}
ROOT_FS_TYPE="$(awk '$2 == "/" { print $3; exit }' /proc/mounts)"
case "${ROOT_FS_TYPE}" in
    tmpfs|overlay|squashfs|ramfs|"")
        echo "ERROR: Alpine diskless/data mode is unsupported (${ROOT_FS_TYPE:-unknown} root)." >&2
        echo "Install Alpine in persistent sys mode to preserve RAM for playback." >&2
        exit 1
        ;;
esac

required_files=(
    "${TARBALL}" "${COMPOSE_FILE}" "${HOST_DIAGNOSTICS_SCRIPT}"
    "${HOST_EVENT_WATCHER_SCRIPT}" "${HOST_REBOOT_SCRIPT}"
    "${HOST_REBOOT_REQUEST_LIB}" "${PROJECT_LICENSE}"
    "${THIRD_PARTY_NOTICES}" "${THIRD_PARTY_SOURCE}"
    "${DEPLOY_TRANSACTION_SCRIPT}" "${DEPLOY_ARTIFACT_MANIFEST}"
)
for service in "${OPENRC_SERVICES[@]}"; do
    required_files+=("${OPENRC_SOURCE_DIR}/${service}")
done
for required_file in "${required_files[@]}"; do
    host_require_regular_file "${required_file}" || {
        echo "ERROR: Required deployment file not found or unsafe: ${required_file}" >&2
        exit 1
    }
done

# Installer state is a fixed, ordered two-key record. Anything else is a
# rejection rather than a partial read: these values decide whether the
# appliance comes back with a picture after an upgrade.
SAVED_HDMI_VIDEO_MODE=auto
SAVED_MAX_VIDEO=auto
if [[ -f "${INSTALLER_CONFIG_FILE}" ]]; then
    mapfile -t installer_config_lines < "${INSTALLER_CONFIG_FILE}"
    # A v0.9.38 or earlier file has only the HDMI line; it predates --max-video
    # and reads as an unset ceiling rather than as corruption.
    case "${#installer_config_lines[@]}" in
        1)
            [[ "${installer_config_lines[0]}" == HDMI_VIDEO_MODE=* ]] || {
                echo "ERROR: Invalid installer state in ${INSTALLER_CONFIG_FILE}." >&2
                exit 1
            }
            ;;
        2)
            [[ "${installer_config_lines[0]}" == HDMI_VIDEO_MODE=* && \
               "${installer_config_lines[1]}" == MAX_VIDEO=* ]] || {
                echo "ERROR: Invalid installer state in ${INSTALLER_CONFIG_FILE}." >&2
                exit 1
            }
            SAVED_MAX_VIDEO="${installer_config_lines[1]#MAX_VIDEO=}"
            ;;
        *)
            echo "ERROR: Invalid installer state in ${INSTALLER_CONFIG_FILE}." >&2
            exit 1
            ;;
    esac
    SAVED_HDMI_VIDEO_MODE="${installer_config_lines[0]#HDMI_VIDEO_MODE=}"
    host_validate_hdmi_video_mode "${SAVED_HDMI_VIDEO_MODE}" || {
        echo "ERROR: Invalid saved HDMI mode." >&2
        exit 1
    }
    [[ "${SAVED_MAX_VIDEO}" == auto ]] || \
        host_validate_video_ceiling "${SAVED_MAX_VIDEO}" || {
            echo "ERROR: Invalid saved video ceiling." >&2
            exit 1
        }
fi
if [[ "${HDMI_VIDEO_EXPLICIT}" == "true" ]]; then
    HDMI_VIDEO_MODE="${HDMI_VIDEO_REQUEST}"
else
    HDMI_VIDEO_MODE="${SAVED_HDMI_VIDEO_MODE}"
fi
if [[ "${MAX_VIDEO_EXPLICIT}" == "true" ]]; then
    MAX_VIDEO="${MAX_VIDEO_REQUEST}"
else
    MAX_VIDEO="${SAVED_MAX_VIDEO}"
fi
OMT_HDMI_CONNECTOR=auto
[[ "${HDMI_VIDEO_MODE}" == auto ]] || OMT_HDMI_CONNECTOR="${HDMI_VIDEO_MODE%%:*}"
# A single-output board has no HDMI-A-2, and a forced mode for it would leave
# the operator with a boot argument for a connector that never appears.
if [[ "${BOARD_HDMI_CONNECTORS}" == "1" && "${OMT_HDMI_CONNECTOR}" == "HDMI-A-2" ]]; then
    echo "ERROR: ${BOARD_LABEL} has one HDMI output; HDMI-A-2 is not available." >&2
    exit 2
fi
# "auto" resolves to the board's tier; an explicit ceiling is the operator's.
OMT_VIDEO_CEILING="${BOARD_VIDEO_CEILING}"
[[ "${MAX_VIDEO}" == auto ]] || OMT_VIDEO_CEILING="${MAX_VIDEO}"
if [[ "${OMT_VIDEO_CEILING}" != "${BOARD_VIDEO_CEILING}" ]]; then
    echo "NOTE: Video ceiling ${OMT_VIDEO_CEILING} overrides the ${BOARD_LABEL} default of ${BOARD_VIDEO_CEILING}."
fi

echo "Updating Alpine packages and installing the appliance dependencies..."
# Match any live community line, not one spelled with a hardcoded series: a
# series-specific pattern never matches after a bump and appends a duplicate
# repository on every install.
if ! grep -Eq '^[^#[:space:]]+/community/?$' /etc/apk/repositories; then
    # Reads the whole file for the same reason the sshd -T pipeline below does:
    # piping into a first-line filter closes the pipe early and trips pipefail.
    MAIN_REPOSITORY="$(awk 'found { next } \
        /^[^#[:space:]]+\/main\/?$/ { sub(/\/main\/?$/, ""); print; found = 1 }' \
        /etc/apk/repositories)"
    [[ "${MAIN_REPOSITORY}" == https://* || "${MAIN_REPOSITORY}" == http://* ]] || {
        echo "ERROR: Enable a trusted Alpine v${SUPPORTED_ALPINE_SERIES} main repository first." >&2
        exit 1
    }
    printf '%s/community\n' "${MAIN_REPOSITORY}" >> /etc/apk/repositories
fi
# Stock Alpine images list HTTP mirrors. Rewrite live lines to HTTPS before
# the first package fetch this installer makes.
if grep -q '^http://' /etc/apk/repositories; then
    echo "Rewriting apk repositories to HTTPS..."
    sed -i -e 's|^http://|https://|' /etc/apk/repositories
fi
# A re-deploy upgrades docker while the appliance is using it. Stop the
# Compose service first so apk is not racing a live container and so a
# docker package restart cannot tear the installer out from under itself.
if [[ -x /etc/init.d/omt-client ]]; then
    echo "Stopping the appliance before updating Alpine packages..."
    rc-service omt-client stop >/dev/null 2>&1 || true
fi
# apk can fetch kernel and firmware images without a newline for longer than
# the deployer's one-minute idle timeout. --progress keeps bytes moving;
# the heartbeat is the fallback when a mirror stalls the meter.
host_apk_upgrade() {
    local heartbeat_pid="" status=0
    echo "Updating Alpine packages to the latest indexed versions..."
    (
        while sleep 20; do
            echo "Alpine package update still running..."
        done
    ) &
    heartbeat_pid=$!
    apk --wait 30 --progress -v update || status=$?
    if (( status == 0 )); then
        apk --wait 30 --progress -v upgrade --available || status=$?
    fi
    kill "${heartbeat_pid}" 2>/dev/null || true
    wait "${heartbeat_pid}" 2>/dev/null || true
    return "${status}"
}
host_apk_upgrade
apk add --no-cache \
    alsa-utils avahi avahi-tools bash coreutils dbus docker docker-cli-compose \
    ethtool findutils inotify-tools iproute2 iw jq libdrm-tests linux-firmware-brcm \
    linux-rpi nftables nftables-rulesets procps raspberrypi-bootloader \
    tcpdump util-linux util-linux-misc wpa_supplicant xdg-dbus-proxy zram-init
echo "Alpine packages are at the latest indexed versions."

# The Windows deployer manages Wi-Fi through wpa_cli. Preserve every existing
# network block while making the control socket and durable save operation an
# explicit part of the Alpine appliance contract.
WPA_SUPPLICANT_DIR=/etc/wpa_supplicant
WPA_SUPPLICANT_CONFIG="${WPA_SUPPLICANT_DIR}/wpa_supplicant.conf"
install -d -m 0700 "${WPA_SUPPLICANT_DIR}"
WPA_CONFIG_TMP="$(mktemp "${WPA_SUPPLICANT_DIR}/.wpa_supplicant.conf.XXXXXX")"
WPA_CONFIG_INPUT=/dev/null
if [[ -e "${WPA_SUPPLICANT_CONFIG}" || -L "${WPA_SUPPLICANT_CONFIG}" ]]; then
    host_require_regular_file "${WPA_SUPPLICANT_CONFIG}" || {
        echo "ERROR: Existing wpa_supplicant configuration is unsafe." >&2
        rm -f -- "${WPA_CONFIG_TMP}"
        exit 1
    }
    WPA_CONFIG_INPUT="${WPA_SUPPLICANT_CONFIG}"
fi
host_wpa_supplicant_config < "${WPA_CONFIG_INPUT}" > "${WPA_CONFIG_TMP}"
host_publish_file "${WPA_SUPPLICANT_CONFIG}" 0600 root root < "${WPA_CONFIG_TMP}"
rm -f -- "${WPA_CONFIG_TMP}"

for command_name in apk docker rc-service rc-update inotifywait xdg-dbus-proxy; do
    command -v "${command_name}" >/dev/null 2>&1 || {
        echo "ERROR: Required Alpine command is missing after package installation: ${command_name}" >&2
        exit 1
    }
done
docker compose version >/dev/null 2>&1 || {
    echo "ERROR: Alpine docker-cli-compose is unavailable." >&2
    exit 1
}

echo "Applying low-memory and operating-system hardening..."
install -d -m 0755 /etc/sysctl.d /etc/docker /etc/ssh/sshd_config.d /etc/nftables.d /etc/modprobe.d
host_publish_file /etc/sysctl.d/90-omt-client-hardening.conf 0644 root root <<'EOF'
# OMT appliance hardening and compressed-memory behavior.
fs.protected_fifos=2
fs.protected_hardlinks=1
fs.protected_regular=2
fs.protected_symlinks=1
fs.suid_dumpable=0
kernel.dmesg_restrict=1
kernel.kptr_restrict=2
kernel.perf_event_paranoid=3
kernel.randomize_va_space=2
kernel.sysrq=0
kernel.unprivileged_bpf_disabled=1
net.core.bpf_jit_harden=2
net.ipv4.conf.all.accept_redirects=0
net.ipv4.conf.all.accept_source_route=0
net.ipv4.conf.all.log_martians=1
net.ipv4.conf.all.secure_redirects=0
net.ipv4.conf.all.send_redirects=0
net.ipv4.conf.default.accept_redirects=0
net.ipv4.conf.default.accept_source_route=0
net.ipv4.conf.default.log_martians=1
net.ipv4.conf.default.secure_redirects=0
net.ipv4.conf.default.send_redirects=0
net.ipv4.icmp_echo_ignore_broadcasts=1
net.ipv4.icmp_ignore_bogus_error_responses=1
net.ipv4.tcp_rfc1337=1
net.ipv4.tcp_syncookies=1
net.ipv4.conf.all.rp_filter=1
net.ipv4.conf.default.rp_filter=1
net.ipv4.conf.all.arp_ignore=1
net.ipv4.conf.all.arp_announce=2
net.ipv4.conf.default.arp_ignore=1
net.ipv4.conf.default.arp_announce=2
net.ipv4.conf.all.drop_gratuitous_arp=1
net.ipv4.tcp_fastopen=3
net.ipv6.conf.all.accept_ra=0
net.ipv6.conf.default.accept_ra=0
net.ipv6.conf.all.autoconf=0
net.ipv6.conf.default.autoconf=0
net.ipv6.conf.all.router_solicitations=0
net.ipv6.conf.all.accept_redirects=0
net.ipv6.conf.all.accept_source_route=0
net.ipv6.conf.default.accept_redirects=0
net.ipv6.conf.default.accept_source_route=0
vm.page-cluster=0
vm.swappiness=100
EOF
sysctl --system >/dev/null

MEMTOTAL_KIB="$(awk '/^MemTotal:/ { print $2; exit }' /proc/meminfo)"
[[ "${MEMTOTAL_KIB}" =~ ^[0-9]+$ ]] || {
    echo "ERROR: Unable to determine installed RAM." >&2
    exit 1
}
ZRAM_MIB=$((MEMTOTAL_KIB / 4096))
(( ZRAM_MIB >= 128 )) || ZRAM_MIB=128
(( ZRAM_MIB <= 512 )) || ZRAM_MIB=512
host_publish_openrc_conf /etc/conf.d/zram-init <<EOF
load_on_start=yes
unload_on_stop=yes
num_devices=1
type0=swap
size0=${ZRAM_MIB}
EOF
rc-update add zram-init default >/dev/null
rc-service zram-init restart >/dev/null 2>&1 || rc-service zram-init start

DOCKER_CONFIG_TMP="$(mktemp /etc/docker/.daemon.json.XXXXXX)"
if [[ -f /etc/docker/daemon.json && ! -L /etc/docker/daemon.json ]]; then
    jq '
        if type != "object" then error("Docker daemon configuration must be an object") else . end
        | .["log-driver"] = "local"
        | .["log-opts"] = ((.["log-opts"] // {}) + {"max-size":"10m","max-file":"3"})
        | .["live-restore"] = true
        | .["userland-proxy"] = false
        | .["no-new-privileges"] = true
    ' /etc/docker/daemon.json > "${DOCKER_CONFIG_TMP}"
else
    jq -n '{"log-driver":"local","log-opts":{"max-size":"10m","max-file":"3"},"live-restore":true,"userland-proxy":false,"no-new-privileges":true}' \
        > "${DOCKER_CONFIG_TMP}"
fi
chmod 0600 "${DOCKER_CONFIG_TMP}"
chown root:root "${DOCKER_CONFIG_TMP}"
# A syntactically valid JSON object may still contain a daemon option Docker
# does not understand. Validate the complete merge before replacing the live
# configuration, otherwise the following restart can take the appliance and
# its recovery path down while leaving the invalid file active for next boot.
dockerd --validate --config-file "${DOCKER_CONFIG_TMP}" >/dev/null
mv -fT "${DOCKER_CONFIG_TMP}" /etc/docker/daemon.json

host_publish_file /etc/ssh/sshd_config.d/90-omt-client-hardening.conf 0644 root root <<'EOF'
# Preserve key-based emergency access while removing risky SSH features.
PermitRootLogin prohibit-password
PasswordAuthentication yes
PubkeyAuthentication yes
KbdInteractiveAuthentication no
AllowGroups root wheel
X11Forwarding no
AllowAgentForwarding no
AllowTcpForwarding no
AllowStreamLocalForwarding no
DisableForwarding yes
GatewayPorts no
HostbasedAuthentication no
IgnoreRhosts yes
PermitEmptyPasswords no
PermitTunnel no
PermitUserEnvironment no
PermitUserRC no
MaxAuthTries 3
MaxSessions 4
MaxStartups 3:30:10
LoginGraceTime 30
ClientAliveInterval 300
ClientAliveCountMax 2
LogLevel VERBOSE
EOF
sshd -t
if rc-service sshd status >/dev/null 2>&1; then
    rc-service sshd reload
fi

# Time stamps the reboot request, TLS certificates, and SSH. Stock Alpine
# already has busybox ntpd when setup-alpine ran; install chrony only when
# nothing is providing a clock.
if [[ -x /etc/init.d/ntpd ]]; then
    rc-update add ntpd default >/dev/null
    rc-service ntpd start >/dev/null 2>&1 || true
elif [[ -x /etc/init.d/chronyd ]]; then
    rc-update add chronyd default >/dev/null
    rc-service chronyd start >/dev/null 2>&1 || true
else
    apk add --no-cache chrony
    rc-update add chronyd default >/dev/null
    rc-service chronyd start
fi

# Onboard Bluetooth is unused by the appliance. Stop a packaged daemon if one
# is present, and block the radio so it cannot come back on a later package.
if [[ -x /etc/init.d/bluetooth ]]; then
    rc-service bluetooth stop >/dev/null 2>&1 || true
    rc-update del bluetooth default >/dev/null 2>&1 || true
fi
if command -v rfkill >/dev/null 2>&1; then
    rfkill block bluetooth >/dev/null 2>&1 || true
fi
host_publish_file /etc/modprobe.d/omt-client-blacklist.conf 0644 root root <<'EOF'
# Onboard Bluetooth is unused. Firmware already disables the controller;
# keep the modules from loading if a later package pulls them in.
blacklist bluetooth
blacklist btbcm
blacklist btintel
blacklist btrtl
blacklist btusb
blacklist hci_uart
install bluetooth /bin/true
EOF

# Decode is software VMX on three cores. schedutil lets the Pi 4 drop below
# the 1080p30 budget; pin performance for the life of the appliance.
# brcmfmac power save also drops mDNS and OMT datagrams, so it is pinned off
# here and again from the wpa_cli CONNECTED hook after every associate.
install -d -m 0755 /etc/local.d
host_publish_file /etc/local.d/omt-client-cpufreq.start 0755 root root <<'EOF'
#!/bin/sh
for gov in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do
    [ -w "${gov}" ] || continue
    echo performance > "${gov}" || true
done
for path in /sys/class/net/*/wireless; do
    [ -d "${path}" ] || continue
    iface=${path#/sys/class/net/}
    iface=${iface%/wireless}
    command -v iw >/dev/null 2>&1 || break
    iw dev "${iface}" set power_save off || true
done
EOF
if [[ -x /etc/init.d/local ]]; then
    rc-update add local default >/dev/null
    /etc/local.d/omt-client-cpufreq.start >/dev/null 2>&1 || true
fi

for desktop_service in lightdm display-manager; do
    rc-service "${desktop_service}" stop >/dev/null 2>&1 || true
    rc-update del "${desktop_service}" default >/dev/null 2>&1 || true
done
rc-update add cgroups default >/dev/null
rc-update add dbus default >/dev/null
rc-update add avahi-daemon default >/dev/null
rc-update add docker default >/dev/null
rc-update add networking boot >/dev/null
rc-update add wpa_supplicant boot >/dev/null
host_publish_file /etc/wpa_supplicant/omt-client-action.sh 0755 root root <<'EOF'
#!/bin/sh
IFACE="${1:-}"
EVENT="${2:-}"
if [ "${EVENT}" = CONNECTED ] && [ -n "${IFACE}" ] && command -v iw >/dev/null 2>&1; then
    iw dev "${IFACE}" set power_save off || true
fi
if [ -x /etc/wpa_supplicant/wpa_cli.sh ]; then
    exec /etc/wpa_supplicant/wpa_cli.sh "$@"
fi
EOF
if [[ -x /etc/init.d/wpa_cli ]]; then
    host_publish_openrc_conf /etc/conf.d/wpa_cli <<'EOF'
WPACLI_OPTS="-a /etc/wpa_supplicant/omt-client-action.sh"
EOF
    rc-update add wpa_cli boot >/dev/null
    rc-service wpa_cli restart >/dev/null 2>&1 || rc-service wpa_cli start >/dev/null 2>&1 || true
fi
rc-service cgroups start >/dev/null 2>&1 || true
rc-service dbus start
rc-service avahi-daemon restart
rc-service docker restart

# OpenRC reports success once dockerd is spawned, not once it is serving. On a
# Pi 4 the daemon still has to restore containers and initialize buildkit
# before it opens /var/run/docker.sock, and every docker command below loses
# that race on a cold install.
DOCKER_API_WAIT_SECONDS=90
DOCKER_READY=false
for _ in $(seq 1 "${DOCKER_API_WAIT_SECONDS}"); do
    if docker info >/dev/null 2>&1; then
        DOCKER_READY=true
        break
    fi
    sleep 1
done
[[ "${DOCKER_READY}" == "true" ]] || {
    echo "ERROR: Docker daemon did not become ready; see /var/log/docker.log." >&2
    exit 1
}

echo "Loading OMT Client image..."
docker load < "${TARBALL}"
OMT_UID="$(docker run --rm --user 0:0 --entrypoint /usr/bin/id omt-client -u omt 2>/dev/null || true)"
OMT_GID="$(docker run --rm --user 0:0 --entrypoint /usr/bin/id omt-client -g omt 2>/dev/null || true)"
[[ "${OMT_UID}" =~ ^[1-9][0-9]*$ && "${OMT_GID}" =~ ^[1-9][0-9]*$ ]] || {
    echo "ERROR: Could not resolve the image-owned OMT UID/GID." >&2
    exit 1
}
IMAGE_ENV="$(docker image inspect --format '{{range .Config.Env}}{{println .}}{{end}}' omt-client)"
WEB_PORT="$(sed -n 's/^WEB_PORT=//p' <<< "${IMAGE_ENV}" | tail -n 1)"
[[ "${WEB_PORT}" =~ ^[0-9]+$ ]] && ((10#${WEB_PORT} >= 1 && 10#${WEB_PORT} <= 65535)) || {
    echo "ERROR: Loaded image has an invalid WEB_PORT." >&2
    exit 1
}

if ! docker volume inspect "${STABLE_VOLUME}" >/dev/null 2>&1; then
    docker volume create "${STABLE_VOLUME}" >/dev/null
fi
docker run --rm --user 0:0 --entrypoint /bin/sh -v "${STABLE_VOLUME}:/config" \
    omt-client -eu -c '
        uid=$1; gid=$2
        chown -R "$uid:$gid" /config
        for file in flask_secret web_secret web_password web_sessions.json web_sessions.lock source_target.json omt/settings.xml ssl/key.pem; do
            [ ! -e "/config/$file" ] || chmod 600 "/config/$file"
        done
        [ ! -e /config/ssl/cert.pem ] || chmod 644 /config/ssl/cert.pem
        rm -rf /config/run
        for directory in /config/ssl /config/omt; do
            [ ! -d "$directory" ] || chmod 700 "$directory"
        done
    ' sh "${OMT_UID}" "${OMT_GID}"

device_gid() {
    local group_name="$1" fallback="$2"
    shift 2
    local device group_entry
    for device in "$@"; do
        if [[ -c "${device}" ]]; then
            stat -c '%g' "${device}"
            return
        fi
    done
    group_entry="$(getent group "${group_name}" 2>/dev/null || true)"
    [[ -z "${group_entry}" ]] || {
        cut -d: -f3 <<< "${group_entry}"
        return
    }
    printf '%s\n' "${fallback}"
}

shopt -s nullglob
VIDEO_CANDIDATES=(/dev/dri/card*)
RENDER_CANDIDATES=(/dev/dri/renderD*)
AUDIO_CANDIDATES=(/dev/snd/*)
AUDIO_PLAYBACK_CANDIDATES=(/dev/snd/pcmC*D*p)
shopt -u nullglob
RUNTIME_DEVICES_READY=true
MISSING_RUNTIME_DEVICES=()
((${#VIDEO_CANDIDATES[@]})) || {
    RUNTIME_DEVICES_READY=false
    MISSING_RUNTIME_DEVICES+=("DRM primary card (/dev/dri/card*)")
}
((${#AUDIO_PLAYBACK_CANDIDATES[@]})) || {
    RUNTIME_DEVICES_READY=false
    MISSING_RUNTIME_DEVICES+=("ALSA playback PCM (/dev/snd/pcmC*D*p)")
}
OMT_VIDEO_GID="$(device_gid video 27 "${VIDEO_CANDIDATES[@]}")"
OMT_RENDER_GID="$(device_gid render "${OMT_VIDEO_GID}" "${RENDER_CANDIDATES[@]}")"
OMT_AUDIO_GID="$(device_gid audio 18 "${AUDIO_PLAYBACK_CANDIDATES[@]}" "${AUDIO_CANDIDATES[@]}")"
for gid in "${OMT_VIDEO_GID}" "${OMT_RENDER_GID}" "${OMT_AUDIO_GID}"; do
    [[ "${gid}" =~ ^[0-9]+$ ]] || {
        echo "ERROR: Invalid device GID discovered: ${gid}" >&2
        exit 1
    }
done
# The render node is not granted separately: on Alpine /dev/dri/renderD* is
# owned by the video group, and Compose rejects a group_add list holding two
# equal items, so a second entry for it stopped the container from starting at
# all. Refuse rather than quietly drop GPU access if that ever stops holding.
[[ "${OMT_RENDER_GID}" == "${OMT_VIDEO_GID}" ]] || {
    echo "ERROR: /dev/dri/renderD* belongs to group ${OMT_RENDER_GID}, not the" >&2
    echo "video group ${OMT_VIDEO_GID}. This appliance grants one DRM group." >&2
    exit 1
}

COMPOSE_ENV_TMP="$(mktemp "${COMPOSE_ENV_FILE}.tmp.XXXXXX")"
{
    printf 'OMT_VIDEO_GID=%s\n' "${OMT_VIDEO_GID}"
    printf 'OMT_AUDIO_GID=%s\n' "${OMT_AUDIO_GID}"
    printf 'OMT_HDMI_CONNECTOR=%s\n' "${OMT_HDMI_CONNECTOR}"
    printf 'OMT_BOARD_LABEL=%s\n' "${BOARD_LABEL}"
    printf 'OMT_VIDEO_CEILING=%s\n' "${OMT_VIDEO_CEILING}"
    printf 'OMT_CONTAINER_MEMORY_LIMIT=128m\n'
} > "${COMPOSE_ENV_TMP}"
chmod 0600 "${COMPOSE_ENV_TMP}"
mv -fT "${COMPOSE_ENV_TMP}" "${COMPOSE_ENV_FILE}"

echo "Installing fixed-purpose OpenRC services..."
install -d -m 0755 "${HOST_COMPONENT_DIR}" /etc/init.d /etc/conf.d
chown root:root "${HOST_COMPONENT_DIR}"
if [[ -x "${DEPLOY_RECOVERY_HELPER}" && \
      -f "${HOST_COMPONENT_DIR}/deploy-artifacts.txt" && \
      ! -L "${HOST_COMPONENT_DIR}/deploy-artifacts.txt" ]]; then
    "${DEPLOY_RECOVERY_HELPER}" recover "${INSTALL_DIR}" \
        "${HOST_COMPONENT_DIR}/deploy-artifacts.txt"
fi
host_publish_file "${HOST_DIAGNOSTICS_INSTALLED_SCRIPT}" 0755 root root < "${HOST_DIAGNOSTICS_SCRIPT}"
host_publish_file "${HOST_EVENT_WATCHER_INSTALLED_SCRIPT}" 0755 root root < "${HOST_EVENT_WATCHER_SCRIPT}"
host_publish_file "${HOST_REBOOT_INSTALLED_SCRIPT}" 0755 root root < "${HOST_REBOOT_SCRIPT}"
host_publish_file "${HOST_REBOOT_REQUEST_LIB_INSTALLED}" 0644 root root < "${HOST_REBOOT_REQUEST_LIB}"
host_publish_file "${DEPLOY_RECOVERY_HELPER}" 0755 root root < "${DEPLOY_TRANSACTION_SCRIPT}"
host_publish_file "${DEPLOY_RECOVERY_MANIFEST}" 0644 root root < "${DEPLOY_ARTIFACT_MANIFEST}"
for service in "${OPENRC_SERVICES[@]}"; do
    rc-service "${service}" stop >/dev/null 2>&1 || true
    host_publish_openrc_service "/etc/init.d/${service}" < "${OPENRC_SOURCE_DIR}/${service}"
done

rm -f -- "${AVAHI_PROXY_SOCKET}"
install -d -m 0755 -o root -g root "${HOST_STATE_DIR}"
# Group-writable, unlike the diagnostics directories below: xdg-dbus-proxy runs
# as ${OMT_UID}:${OMT_GID} and has to *create* its socket in here. At 0750 the
# group got r-x, so every install ended with "Error binding to address
# (GUnixSocketAddress): Permission denied" and a proxy that never came up.
# Setgid so the socket carries the group the container is granted through.
install -d -m 2770 -o root -g "${OMT_GID}" "${AVAHI_STATE_DIR}"
install -d -m 2750 -o root -g "${OMT_GID}" "${HOST_DIAGNOSTICS_STATE_DIR}" "${HOST_ACTION_STATE_DIR}"
touch "${HOST_DIAGNOSTICS_REQUEST_FILE}" "${HOST_DIAGNOSTICS_REPORT_FILE}" \
    "${HOST_REBOOT_REQUEST_FILE}" "${HOST_REBOOT_RESULT_FILE}"
chown "${OMT_UID}:${OMT_GID}" "${HOST_DIAGNOSTICS_REQUEST_FILE}" "${HOST_REBOOT_REQUEST_FILE}"
chown "root:${OMT_GID}" "${HOST_DIAGNOSTICS_REPORT_FILE}" "${HOST_REBOOT_RESULT_FILE}"
chmod 0600 "${HOST_DIAGNOSTICS_REQUEST_FILE}" "${HOST_REBOOT_REQUEST_FILE}"
chmod 0640 "${HOST_DIAGNOSTICS_REPORT_FILE}" "${HOST_REBOOT_RESULT_FILE}"

host_publish_openrc_conf /etc/conf.d/omt-client <<EOF
OMT_INSTALL_DIR=${INSTALL_DIR}
OMT_COMPOSE_FILE=${COMPOSE_FILE}
OMT_COMPOSE_ENV_FILE=${COMPOSE_ENV_FILE}
OMT_RECOVERY_HELPER=${DEPLOY_RECOVERY_HELPER}
OMT_RECOVERY_MANIFEST=${DEPLOY_RECOVERY_MANIFEST}
OMT_DOCKER_API_WAIT_SECONDS=${DOCKER_API_WAIT_SECONDS}
EOF
host_publish_openrc_conf /etc/conf.d/omt-client-avahi-proxy <<EOF
OMT_AVAHI_PROXY_SOCKET=${AVAHI_PROXY_SOCKET}
OMT_UID=${OMT_UID}
OMT_GID=${OMT_GID}
EOF
host_publish_openrc_conf /etc/conf.d/omt-client-host-diagnostics <<EOF
export OMT_INSTALL_DIR=${INSTALL_DIR}
export OMT_DIAGNOSTICS_HOST_BUDGET_SECONDS=25
export OMT_DIAGNOSTICS_HOST_REPORT_FILE=${HOST_DIAGNOSTICS_REPORT_FILE}
export OMT_DIAGNOSTICS_HOST_REQUEST_FILE=${HOST_DIAGNOSTICS_REQUEST_FILE}
export OMT_DIAGNOSTICS_HOST_PCAP_FILE=${HOST_DIAGNOSTICS_STATE_DIR}/host-network.pcap
export OMT_DIAGNOSTICS_HOST_PCAP_METADATA_FILE=${HOST_DIAGNOSTICS_STATE_DIR}/host-network-pcap.txt
export OMT_DIAGNOSTICS_ACTION=${HOST_DIAGNOSTICS_INSTALLED_SCRIPT}
EOF
host_publish_openrc_conf /etc/conf.d/omt-client-reboot <<EOF
export OMT_UID=${OMT_UID}
export OMT_GID=${OMT_GID}
export OMT_REBOOT_REQUEST_FILE=${HOST_REBOOT_REQUEST_FILE}
export OMT_REBOOT_RESULT_FILE=${HOST_REBOOT_RESULT_FILE}
export OMT_REBOOT_ACTION=${HOST_REBOOT_INSTALLED_SCRIPT}
EOF

echo "Configuring Alpine Pi KMS and HDMI audio..."
BOOT_ROOT=""
for candidate in /media/mmcblk0p1 /boot /media/*; do
    if [[ -d "${candidate}" && ! -L "${candidate}" && \
          ( -f "${candidate}/config.txt" || -f "${candidate}/usercfg.txt" ) ]]; then
        BOOT_ROOT="${candidate}"
        break
    fi
done
[[ -n "${BOOT_ROOT}" ]] || {
    echo "ERROR: Could not locate the active Alpine Raspberry Pi boot partition." >&2
    exit 1
}
USERCFG_FILE="${BOOT_ROOT}/usercfg.txt"
HDMI_TMP="$(mktemp "${USERCFG_FILE}.omt-client.XXXXXX")"
if [[ -e "${USERCFG_FILE}" || -L "${USERCFG_FILE}" ]]; then
    [[ -f "${USERCFG_FILE}" && ! -L "${USERCFG_FILE}" && -r "${USERCFG_FILE}" ]] || {
        echo "ERROR: Alpine usercfg.txt must be a readable regular file." >&2
        rm -f -- "${HDMI_TMP}"
        exit 1
    }
    host_hdmi_config_txt "${BOARD_ID}" < "${USERCFG_FILE}" > "${HDMI_TMP}"
    chmod --reference="${USERCFG_FILE}" "${HDMI_TMP}"
    chown --reference="${USERCFG_FILE}" "${HDMI_TMP}"
else
    host_hdmi_config_txt "${BOARD_ID}" < /dev/null > "${HDMI_TMP}"
    chmod 0644 "${HDMI_TMP}"
    chown root:root "${HDMI_TMP}"
fi
mv -fT "${HDMI_TMP}" "${USERCFG_FILE}"

CMDLINE_FILE=""
for candidate in "${BOOT_ROOT}/cmdline.txt" "${BOOT_ROOT}/cmdline-rpi.txt" "${BOOT_ROOT}/cmdline-rpi2.txt"; do
    if [[ -f "${candidate}" && ! -L "${candidate}" ]]; then
        CMDLINE_FILE="${candidate}"
        break
    fi
done
[[ -n "${CMDLINE_FILE}" ]] || {
    echo "ERROR: An active regular boot cmdline file is required to enable the memory cgroup." >&2
    exit 1
}
PREVIOUS_HDMI_TOKEN=""
[[ "${SAVED_HDMI_VIDEO_MODE}" == auto ]] || PREVIOUS_HDMI_TOKEN="video=${SAVED_HDMI_VIDEO_MODE}D"
DESIRED_HDMI_TOKEN=""
DESIRED_HDMI_CONNECTOR=""
if [[ "${HDMI_VIDEO_MODE}" != auto ]]; then
    DESIRED_HDMI_TOKEN="video=${HDMI_VIDEO_MODE}D"
    DESIRED_HDMI_CONNECTOR="${HDMI_VIDEO_MODE%%:*}"
fi
mapfile -t cmdline_lines < "${CMDLINE_FILE}"
[[ "${#cmdline_lines[@]}" -eq 1 && -n "${cmdline_lines[0]}" ]] || {
    echo "ERROR: ${CMDLINE_FILE} must contain one non-empty line." >&2
    exit 1
}
UPDATED_CMDLINE="$(host_hdmi_cmdline_line "${cmdline_lines[0]}" \
    "${PREVIOUS_HDMI_TOKEN}" "${DESIRED_HDMI_TOKEN}" "${DESIRED_HDMI_CONNECTOR}")" || {
    echo "ERROR: ${CMDLINE_FILE} contains an unmanaged connector mode." >&2
    exit 1
}
UPDATED_CMDLINE="$(host_cmdline_memory_cgroup "${UPDATED_CMDLINE}")"
if [[ "${UPDATED_CMDLINE}" != "${cmdline_lines[0]}" ]]; then
    CMDLINE_TMP="$(mktemp "${CMDLINE_FILE}.omt-client.XXXXXX")"
    printf '%s\n' "${UPDATED_CMDLINE}" > "${CMDLINE_TMP}"
    chmod --reference="${CMDLINE_FILE}" "${CMDLINE_TMP}"
    chown --reference="${CMDLINE_FILE}" "${CMDLINE_TMP}"
    mv -fT "${CMDLINE_TMP}" "${CMDLINE_FILE}"
fi
install -d -m 0755 "${INSTALLER_CONFIG_DIR}"
host_publish_file "${INSTALLER_CONFIG_FILE}" 0644 root root <<EOF
HDMI_VIDEO_MODE=${HDMI_VIDEO_MODE}
MAX_VIDEO=${MAX_VIDEO}
EOF

echo "Enabling the appliance firewall..."
# awk must drain sshd -T rather than `exit` on the first match: closing the pipe
# early kills sshd with SIGPIPE, and under `set -o pipefail` that 141 propagates
# out of the command substitution and aborts the installer with no message at
# all, right after the last thing it printed.
SSH_PORT="$(sshd -T 2>/dev/null | \
    awk '$1 == "port" && !seen { port = $2; seen = 1 } END { if (seen) print port }')"
[[ "${SSH_PORT}" =~ ^[0-9]+$ ]] || SSH_PORT=22
# The appliance's rules go in the host's own input chain, not in a private
# table. A separate table hooked at a lower priority does not work: netfilter
# runs every base chain registered on a hook, an `accept` only ends the chain it
# is in, and Alpine's stock /etc/nftables.nft ends its input chain with
# `policy drop`. Hooking a second table at priority -20 therefore accepted SSH
# and the web UI and then had them dropped at priority 0, leaving the appliance
# unreachable on every port -- recoverable only with a console or the SD card.
#
# The first block creates the chain on a host that has no ruleset of its own and
# is a no-op when Alpine's already exists. The second appends, so any rules the
# host already had keep their position ahead of these.
host_publish_file /etc/nftables.d/omt-client.nft 0600 root root <<EOF
table inet filter {
    chain input {
        type filter hook input priority 0; policy drop;
    }
}
table inet filter {
    chain input {
        ct state established,related accept
        ct state invalid drop
        iifname "lo" accept
        ip protocol icmp accept
        ip6 nexthdr ipv6-icmp accept
        udp sport 67 udp dport 68 accept
        udp sport 547 udp dport 546 accept
        udp dport 5353 accept
        tcp dport { ${SSH_PORT}, ${WEB_PORT} } accept
    }
}
EOF
if ! grep -Fq 'include "/etc/nftables.d/*.nft"' /etc/nftables.nft; then
    printf '\ninclude "/etc/nftables.d/*.nft"\n' >> /etc/nftables.nft
fi
nft -c -f /etc/nftables.nft
rc-update add nftables boot >/dev/null
rc-service nftables restart

for service in omt-client-avahi-proxy omt-client-host-diagnostics omt-client-reboot omt-client; do
    rc-update add "${service}" default >/dev/null
done
rc-service omt-client-avahi-proxy restart || \
    echo "WARNING: Filtered Avahi discovery proxy will retry under OpenRC."
rc-service omt-client-host-diagnostics restart
rc-service omt-client-reboot restart

STARTUP_DEFERRED=false
if [[ "${RUNTIME_DEVICES_READY}" == "true" ]]; then
    rc-service omt-client restart
else
    STARTUP_DEFERRED=true
    printf 'Container startup deferred; missing runtime devices: %s\n' \
        "$(IFS=', '; echo "${MISSING_RUNTIME_DEVICES[*]}")"
    echo "The enabled OpenRC service will retry after the required reboot."
fi

# Release-v1 flat names are never inputs to v2 after its helper is durable.
rm -f -- \
    "${INSTALL_DIR}/docker-compose.yml" "${INSTALL_DIR}/install.sh" \
    "${INSTALL_DIR}/uninstall.sh" "${INSTALL_DIR}/host-debug.sh" \
    "${INSTALL_DIR}/host-reboot.sh" "${INSTALL_DIR}/deploy-transaction.sh" \
    "${INSTALL_DIR}/deploy-artifacts.txt" \
    "${HOST_COMPONENT_DIR}/host-debug.sh" "${HOST_COMPONENT_DIR}/deploy-artifacts.txt"

IP_ADDR="$(host_primary_ipv4)"
[[ -n "${IP_ADDR}" ]] || IP_ADDR="$(hostname 2>/dev/null || echo raspberrypi)"
echo
echo "=== Installation Complete ==="
echo "Platform: Alpine Linux ${SUPPORTED_ALPINE_SERIES} aarch64 on ${PI_MODEL}"
echo "Install:  ${INSTALL_DIR}"
echo "Web UI:   https://${IP_ADDR}:${WEB_PORT}$([[ "${STARTUP_DEFERRED}" == true ]] && echo ' (after reboot)')"
echo "HDMI:     ${HDMI_VIDEO_MODE} (${BOARD_HDMI_CONNECTORS} output(s))"
echo "Video:    up to ${OMT_VIDEO_CEILING}"
echo "Security: nftables, SSH safeguards, kernel hardening, bounded Docker logs"
echo "Memory:   ${ZRAM_MIB} MiB zram swap, 128 MiB container cap, bounded tmpfs"
echo "Packages: Alpine apk update and upgrade --available applied"
echo
echo "Password: after startup, run:"
echo "  sudo docker compose --env-file ${COMPOSE_ENV_FILE} -f ${COMPOSE_FILE} logs omt-client | grep -A 1 'Web UI password'"
echo "Status: sudo rc-service omt-client status"
echo "Logs:   sudo docker compose --env-file ${COMPOSE_ENV_FILE} -f ${COMPOSE_FILE} logs -f omt-client"
echo
echo "A reboot is required to load the updated Pi kernel/firmware and KMS settings."
# `make deploy` and the egui deployer both run this over a non-interactive SSH
# channel, where `read` sees EOF immediately and returns non-zero -- under
# `set -e` that turned a fully successful install into a failed one.
REBOOT_CHOICE=n
if [[ -t 0 ]]; then
    # `ssh -t` gives a tty even when no human is attached, so the read can still
    # hit EOF. Defaulting on failure keeps that from aborting an install that
    # has already fully succeeded.
    read -r -p "Reboot now? (y/N): " REBOOT_CHOICE || REBOOT_CHOICE=n
else
    echo "Non-interactive install; the deployment client will reboot the Pi."
fi
if [[ "${REBOOT_CHOICE}" =~ ^[Yy] ]]; then
    /sbin/reboot
fi
