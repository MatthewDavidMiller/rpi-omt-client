#!/bin/bash
# RPi OMT Client Installer
# Usage: sudo /absolute/path/to/omt-client/install.sh \
#            [--hdmi-video auto|HDMI-A-[12]:WIDTHxHEIGHT@HZ]

set -euo pipefail

export LC_ALL=C
umask 022

usage() {
    cat <<'EOF'
Usage: install.sh [--hdmi-video MODE]

MODE is "auto" or a KMS connector and mode such as:
  HDMI-A-1:1920x1080@60
  HDMI-A-2:1280x720@60

With no option, a previously saved choice is preserved; first installs use auto.
EOF
}

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
INSTALL_DIR="$(cd -- "${SCRIPT_DIR}/../.." && pwd -P)"
LIB_DIR="${INSTALL_DIR}/deploy/lib"
# shellcheck source=deploy/lib/host-validation.sh
source "${LIB_DIR}/host-validation.sh"
# shellcheck source=deploy/lib/hdmi-config.sh
source "${LIB_DIR}/hdmi-config.sh"
# shellcheck source=deploy/lib/publication.sh
source "${LIB_DIR}/publication.sh"
# shellcheck source=deploy/lib/service-install.sh
source "${LIB_DIR}/service-install.sh"

HDMI_VIDEO_EXPLICIT=false
HDMI_VIDEO_REQUEST=""
while (($#)); do
    case "$1" in
        --hdmi-video)
            [[ $# -ge 2 ]] || {
                echo "ERROR: --hdmi-video requires a value." >&2
                usage >&2
                exit 2
            }
            [[ "${HDMI_VIDEO_EXPLICIT}" == "false" ]] || {
                echo "ERROR: --hdmi-video may only be specified once." >&2
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

if ! host_validate_safe_absolute_path "${INSTALL_DIR}"; then
    echo "ERROR: Invalid install directory: ${INSTALL_DIR}" >&2
    exit 1
fi

TARBALL="${INSTALL_DIR}/omt-client-arm64.tar.gz"
COMPOSE_FILE="${INSTALL_DIR}/deploy/compose.yml"
COMPOSE_ENV_FILE="${INSTALL_DIR}/deploy/.env"
HOST_DIAGNOSTICS_SCRIPT="${INSTALL_DIR}/deploy/host/host-diagnostics.sh"
HOST_REBOOT_SCRIPT="${INSTALL_DIR}/deploy/host/host-reboot.sh"
PROJECT_LICENSE="${INSTALL_DIR}/LICENSE"
THIRD_PARTY_NOTICES="${INSTALL_DIR}/THIRD_PARTY_NOTICES.txt"
THIRD_PARTY_SOURCE="${INSTALL_DIR}/THIRD_PARTY_SOURCE.md"
DEPLOY_TRANSACTION_SCRIPT="${INSTALL_DIR}/deploy/transaction.sh"
DEPLOY_ARTIFACT_MANIFEST="${INSTALL_DIR}/deploy/manifest-v2.txt"
HOST_COMPONENT_DIR="/usr/local/libexec/omt-client"
HOST_DIAGNOSTICS_INSTALLED_SCRIPT="${HOST_COMPONENT_DIR}/host-diagnostics.sh"
HOST_REBOOT_INSTALLED_SCRIPT="${HOST_COMPONENT_DIR}/host-reboot.sh"
DEPLOY_RECOVERY_HELPER="${HOST_COMPONENT_DIR}/recover-deployment.sh"
DEPLOY_RECOVERY_MANIFEST="${HOST_COMPONENT_DIR}/manifest-v2.txt"
STABLE_VOLUME="omt-config"
HOST_STATE_DIR="/var/lib/omt-client"
AVAHI_STATE_DIR="${HOST_STATE_DIR}/avahi"
HOST_DIAGNOSTICS_STATE_DIR="${HOST_STATE_DIR}/diagnostics"
HOST_ACTION_STATE_DIR="${HOST_STATE_DIR}/host-actions"
HOST_DIAGNOSTICS_REQUEST_FILE="${HOST_DIAGNOSTICS_STATE_DIR}/request"
HOST_DIAGNOSTICS_REPORT_FILE="${HOST_DIAGNOSTICS_STATE_DIR}/host-report.txt"
HOST_REBOOT_REQUEST_FILE="${HOST_ACTION_STATE_DIR}/reboot.request"
HOST_REBOOT_RESULT_FILE="${HOST_ACTION_STATE_DIR}/reboot.result"
AVAHI_PROXY_SOCKET="${AVAHI_STATE_DIR}/system-bus"
AVAHI_PROXY_SERVICE_FILE="/etc/systemd/system/omt-client-avahi-proxy.service"
HOST_DIAGNOSTICS_SERVICE_FILE="/etc/systemd/system/omt-client-host-diagnostics.service"
HOST_DIAGNOSTICS_PATH_FILE="/etc/systemd/system/omt-client-host-diagnostics.path"
HOST_REBOOT_SERVICE_FILE="/etc/systemd/system/omt-client-reboot.service"
HOST_REBOOT_PATH_FILE="/etc/systemd/system/omt-client-reboot.path"
INSTALLER_CONFIG_DIR="/etc/omt-client"
INSTALLER_CONFIG_FILE="${INSTALLER_CONFIG_DIR}/installer.conf"
CMDLINE_FILE="/boot/firmware/cmdline.txt"
LEGACY_HOST_DEBUG_SERVICE_FILE="/etc/systemd/system/omt-client-host-debug.service"
LEGACY_HOST_DEBUG_PATH_FILE="/etc/systemd/system/omt-client-host-debug.path"
LEGACY_HOST_DEBUG_TIMER_FILE="/etc/systemd/system/omt-client-host-debug.timer"

echo "=== RPi OMT Client Installer ==="

# ─── Pre-flight checks ───────────────────────────────────────────────────────

ARCH="$(uname -m)"
if [[ "${ARCH}" != "aarch64" ]]; then
    echo "ERROR: This installer requires a 64-bit ARM system (aarch64). Detected: ${ARCH}"
    exit 1
fi

if [[ "${EUID}" -ne 0 ]]; then
    echo "ERROR: Please run as root (sudo ${INSTALL_DIR}/install.sh)"
    exit 1
fi

for required_file in "${TARBALL}" "${COMPOSE_FILE}" "${HOST_DIAGNOSTICS_SCRIPT}" \
        "${HOST_REBOOT_SCRIPT}" "${PROJECT_LICENSE}" "${THIRD_PARTY_NOTICES}" \
        "${THIRD_PARTY_SOURCE}" "${DEPLOY_TRANSACTION_SCRIPT}" \
        "${DEPLOY_ARTIFACT_MANIFEST}"; do
    if ! host_require_regular_file "${required_file}"; then
        echo "ERROR: Required deployment file not found: ${required_file}"
        exit 1
    fi
done

legacy_paths=(
    /etc/systemd/system/ndi-client.service
    /etc/systemd/system/ndi-client-host-debug.service
    /etc/systemd/system/ndi-client-host-debug.path
    /etc/ndi-client
    /var/lib/ndi-client
    /usr/local/libexec/ndi-client
)
for legacy_path in "${legacy_paths[@]}"; do
    if [[ -e "${legacy_path}" || -L "${legacy_path}" ]]; then
        echo "ERROR: A legacy NDI Client installation was detected at ${legacy_path}." >&2
        echo "Uninstall the legacy product first; this OMT release does not migrate or modify it." >&2
        exit 1
    fi
done
if command -v docker >/dev/null 2>&1; then
    if docker container inspect ndi-client >/dev/null 2>&1 || \
       docker volume inspect ndi-config >/dev/null 2>&1 || \
       docker volume inspect ndi-client_ndi-config >/dev/null 2>&1; then
        echo "ERROR: Legacy NDI Client Docker resources were detected." >&2
        echo "Uninstall the legacy product first; this OMT release does not migrate or modify it." >&2
        exit 1
    fi
fi

SAVED_HDMI_VIDEO_MODE="auto"
if [[ -f "${INSTALLER_CONFIG_FILE}" ]]; then
    mapfile -t installer_config_lines < "${INSTALLER_CONFIG_FILE}"
    if [[ "${#installer_config_lines[@]}" -ne 1 || \
          "${installer_config_lines[0]}" != HDMI_VIDEO_MODE=* ]]; then
        echo "ERROR: Invalid installer state in ${INSTALLER_CONFIG_FILE}." >&2
        exit 1
    fi
    SAVED_HDMI_VIDEO_MODE="${installer_config_lines[0]#HDMI_VIDEO_MODE=}"
    if ! host_validate_hdmi_video_mode "${SAVED_HDMI_VIDEO_MODE}"; then
        echo "ERROR: Invalid saved HDMI mode in ${INSTALLER_CONFIG_FILE}." >&2
        exit 1
    fi
fi
if [[ "${HDMI_VIDEO_EXPLICIT}" == "true" ]]; then
    HDMI_VIDEO_MODE="${HDMI_VIDEO_REQUEST}"
else
    HDMI_VIDEO_MODE="${SAVED_HDMI_VIDEO_MODE}"
fi
OMT_HDMI_CONNECTOR="auto"
if [[ "${HDMI_VIDEO_MODE}" != "auto" ]]; then
    OMT_HDMI_CONNECTOR="${HDMI_VIDEO_MODE%%:*}"
fi

# ─── Disable desktop environment ─────────────────────────────────────────────

echo "Configuring headless boot..."
systemctl set-default multi-user.target
for unit in display-manager.service lightdm.service; do
    UNIT_FILE="$(systemctl list-unit-files "${unit}" --no-legend 2>/dev/null || true)"
    if [[ "${UNIT_FILE}" == "${unit}"* ]]; then
        systemctl disable "${unit}" || true
        systemctl stop "${unit}" || true
    fi
done

# ─── Install and activate Docker ─────────────────────────────────────────────

if ! command -v docker >/dev/null 2>&1; then
    echo "Installing Docker..."
    apt-get update
    apt-get install -y ca-certificates curl
    install -m 0755 -d /etc/apt/keyrings
    curl -fsSL https://download.docker.com/linux/debian/gpg -o /etc/apt/keyrings/docker.asc
    chmod a+r /etc/apt/keyrings/docker.asc
    # shellcheck source=/etc/os-release
    . /etc/os-release
    VERSION_CODENAME="${VERSION_CODENAME:?VERSION_CODENAME missing from /etc/os-release}"
    DOCKER_ARCH="$(dpkg --print-architecture)"
    printf '%s\n' \
        "deb [arch=${DOCKER_ARCH} signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/debian ${VERSION_CODENAME} stable" \
        > /etc/apt/sources.list.d/docker.list
    apt-get update
    apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
else
    echo "Docker already installed."
fi

systemctl enable --now docker
if [[ -n "${SUDO_USER:-}" && "${SUDO_USER}" != "root" ]] && id "${SUDO_USER}" >/dev/null 2>&1; then
    usermod -aG docker "${SUDO_USER}"
fi

if ! docker compose version >/dev/null 2>&1; then
    echo "Installing docker-compose-plugin..."
    apt-get update
    apt-get install -y docker-compose-plugin
fi
if ! docker compose version >/dev/null 2>&1; then
    echo "ERROR: Docker Compose plugin is unavailable after installation." >&2
    exit 1
fi

# ─── Install host discovery and diagnostics services ────────────────────────

if ! command -v tcpdump >/dev/null 2>&1 || ! command -v avahi-browse >/dev/null 2>&1 || \
   ! command -v avahi-daemon >/dev/null 2>&1 || \
   ! command -v xdg-dbus-proxy >/dev/null 2>&1 || \
   ! command -v ip >/dev/null 2>&1 || ! command -v ss >/dev/null 2>&1 || \
   ! command -v bridge >/dev/null 2>&1 || ! command -v tc >/dev/null 2>&1 || \
   ! command -v iw >/dev/null 2>&1 || ! command -v ethtool >/dev/null 2>&1 || \
   ! command -v rfkill >/dev/null 2>&1 || \
   [[ ! -S /run/dbus/system_bus_socket ]]; then
    echo "Installing host network services and diagnostics..."
    apt-get update
    apt-get install -y tcpdump avahi-daemon avahi-utils dbus xdg-dbus-proxy \
        iproute2 iw ethtool rfkill
fi
systemctl start dbus
systemctl enable --now avahi-daemon

# ─── Load image and resolve image-owned runtime settings ────────────────────

echo "Loading Docker image from ${TARBALL}..."
docker load < "${TARBALL}"

OMT_UID="$(docker run --rm --user 0:0 --entrypoint /usr/bin/id omt-client -u omt 2>/dev/null || true)"
OMT_GID="$(docker run --rm --user 0:0 --entrypoint /usr/bin/id omt-client -g omt 2>/dev/null || true)"
if [[ ! "${OMT_UID}" =~ ^[1-9][0-9]*$ || ! "${OMT_GID}" =~ ^[1-9][0-9]*$ ]]; then
    echo "ERROR: Could not resolve the numeric omt UID/GID from the loaded image." >&2
    exit 1
fi

IMAGE_ENV="$(docker image inspect --format '{{range .Config.Env}}{{println .}}{{end}}' omt-client)"
WEB_PORT="$(sed -n 's/^WEB_PORT=//p' <<< "${IMAGE_ENV}" | tail -n 1)"
if [[ ! "${WEB_PORT}" =~ ^[0-9]+$ ]] || \
   (( 10#${WEB_PORT} < 1 || 10#${WEB_PORT} > 65535 )); then
    echo "ERROR: Loaded image has an invalid WEB_PORT value: ${WEB_PORT:-<missing>}" >&2
    exit 1
fi

# ─── Create the stable persistent volume ────────────────────────────────────

if ! docker volume inspect "${STABLE_VOLUME}" >/dev/null 2>&1; then
    docker volume create "${STABLE_VOLUME}" >/dev/null
fi

docker run --rm --user 0:0 \
    --entrypoint /bin/sh \
    -v "${STABLE_VOLUME}:/config" \
    omt-client -eu -c '
        uid=$1
        gid=$2
        chown -R "$uid:$gid" /config
        for file in flask_secret web_password web_sessions.json web_sessions.lock source_target.json omt/settings.xml ssl/key.pem; do
            [ ! -e "/config/$file" ] || chmod 600 "/config/$file"
        done
        [ ! -e /config/ssl/cert.pem ] || chmod 644 /config/ssl/cert.pem
        for directory in /config/ssl /config/run /config/omt; do
            [ ! -d "$directory" ] || chmod 700 "$directory"
        done
    ' sh "${OMT_UID}" "${OMT_GID}"

# ─── Discover host device groups and write Compose substitutions ─────────────

device_gid() {
    local group_name="$1"
    local fallback="$2"
    shift 2
    local device group_entry

    for device in "$@"; do
        if [[ -c "${device}" ]]; then
            stat -c '%g' "${device}"
            return 0
        fi
    done
    group_entry="$(getent group "${group_name}" 2>/dev/null || true)"
    if [[ -n "${group_entry}" ]]; then
        cut -d: -f3 <<< "${group_entry}"
        return 0
    fi
    printf '%s\n' "${fallback}"
}

shopt -s nullglob
VIDEO_CANDIDATES=(/dev/dri/card*)
RENDER_CANDIDATES=(/dev/dri/renderD*)
AUDIO_CANDIDATES=(/dev/snd/*)
AUDIO_PLAYBACK_CANDIDATES=(/dev/snd/pcmC*D*p)
shopt -u nullglob
VIDEO_DEVICES=()
RENDER_DEVICES=()
AUDIO_DEVICES=()
AUDIO_PLAYBACK_DEVICES=()
for device in "${VIDEO_CANDIDATES[@]}"; do
    [[ -c "${device}" ]] && VIDEO_DEVICES+=("${device}")
done
for device in "${RENDER_CANDIDATES[@]}"; do
    [[ -c "${device}" ]] && RENDER_DEVICES+=("${device}")
done
for device in "${AUDIO_CANDIDATES[@]}"; do
    [[ -c "${device}" ]] && AUDIO_DEVICES+=("${device}")
done
for device in "${AUDIO_PLAYBACK_CANDIDATES[@]}"; do
    [[ -c "${device}" ]] && AUDIO_PLAYBACK_DEVICES+=("${device}")
done

RUNTIME_DEVICES_READY=true
MISSING_RUNTIME_DEVICES=()
if ((${#VIDEO_DEVICES[@]} == 0)); then
    RUNTIME_DEVICES_READY=false
    MISSING_RUNTIME_DEVICES+=("DRM primary card (/dev/dri/card*)")
fi
if ((${#AUDIO_PLAYBACK_DEVICES[@]} == 0)); then
    RUNTIME_DEVICES_READY=false
    MISSING_RUNTIME_DEVICES+=("ALSA playback PCM (/dev/snd/pcmC*D*p)")
fi

OMT_VIDEO_GID="$(device_gid video 44 "${VIDEO_DEVICES[@]}")"
OMT_RENDER_GID="$(device_gid render "${OMT_VIDEO_GID}" "${RENDER_DEVICES[@]}")"
OMT_AUDIO_GID="$(device_gid audio 29 "${AUDIO_PLAYBACK_DEVICES[@]}" "${AUDIO_DEVICES[@]}")"
for gid in "${OMT_VIDEO_GID}" "${OMT_RENDER_GID}" "${OMT_AUDIO_GID}"; do
    if [[ ! "${gid}" =~ ^[0-9]+$ ]]; then
        echo "ERROR: Invalid device group ID discovered: ${gid}" >&2
        exit 1
    fi
done

COMPOSE_ENV_TMP="$(mktemp "${COMPOSE_ENV_FILE}.tmp.XXXXXX")"
{
    printf 'OMT_VIDEO_GID=%s\n' "${OMT_VIDEO_GID}"
    printf 'OMT_RENDER_GID=%s\n' "${OMT_RENDER_GID}"
    printf 'OMT_AUDIO_GID=%s\n' "${OMT_AUDIO_GID}"
    printf 'OMT_HDMI_CONNECTOR=%s\n' "${OMT_HDMI_CONNECTOR}"
} > "${COMPOSE_ENV_TMP}"
chmod 644 "${COMPOSE_ENV_TMP}"
sync -f "${COMPOSE_ENV_TMP}"
mv -fT "${COMPOSE_ENV_TMP}" "${COMPOSE_ENV_FILE}"
sync -d "$(dirname -- "${COMPOSE_ENV_FILE}")"

# ─── Configure request-triggered host diagnostics ───────────────────────────

install -d -m 0755 "${HOST_COMPONENT_DIR}"
chown root:root "${HOST_COMPONENT_DIR}"

# A v1 helper must settle its own flat journal before v2 code can create a
# nested-path journal. Its installed v1 manifest is the only valid rollback map.
if [[ -x "${DEPLOY_RECOVERY_HELPER}" && \
      -f "${HOST_COMPONENT_DIR}/deploy-artifacts.txt" && \
      ! -L "${HOST_COMPONENT_DIR}/deploy-artifacts.txt" ]]; then
    "${DEPLOY_RECOVERY_HELPER}" recover "${INSTALL_DIR}" \
        "${HOST_COMPONENT_DIR}/deploy-artifacts.txt"
fi

host_publish_file "${HOST_DIAGNOSTICS_INSTALLED_SCRIPT}" 0755 root root \
    < "${HOST_DIAGNOSTICS_SCRIPT}"
host_publish_file "${HOST_REBOOT_INSTALLED_SCRIPT}" 0755 root root \
    < "${HOST_REBOOT_SCRIPT}"
host_publish_file "${DEPLOY_RECOVERY_HELPER}" 0755 root root \
    < "${DEPLOY_TRANSACTION_SCRIPT}"
host_publish_file "${DEPLOY_RECOVERY_MANIFEST}" 0644 root root \
    < "${DEPLOY_ARTIFACT_MANIFEST}"
systemctl stop omt-client-avahi-proxy.service 2>/dev/null || true
rm -f "${AVAHI_PROXY_SOCKET}"
install -d -m 0755 -o root -g root "${HOST_STATE_DIR}"
install -d -m 0750 -o root -g "${OMT_GID}" "${AVAHI_STATE_DIR}"
install -d -m 2750 -o root -g "${OMT_GID}" "${HOST_DIAGNOSTICS_STATE_DIR}"
touch "${HOST_DIAGNOSTICS_REQUEST_FILE}" "${HOST_DIAGNOSTICS_REPORT_FILE}"
chown "${OMT_UID}:${OMT_GID}" "${HOST_DIAGNOSTICS_REQUEST_FILE}"
chown "root:${OMT_GID}" "${HOST_DIAGNOSTICS_REPORT_FILE}"
chmod 600 "${HOST_DIAGNOSTICS_REQUEST_FILE}"
chmod 640 "${HOST_DIAGNOSTICS_REPORT_FILE}"
install -d -m 2750 -o root -g "${OMT_GID}" "${HOST_ACTION_STATE_DIR}"
touch "${HOST_REBOOT_REQUEST_FILE}" "${HOST_REBOOT_RESULT_FILE}"
chown "${OMT_UID}:${OMT_GID}" "${HOST_REBOOT_REQUEST_FILE}"
chown "root:${OMT_GID}" "${HOST_REBOOT_RESULT_FILE}"
chmod 0600 "${HOST_REBOOT_REQUEST_FILE}"
chmod 0640 "${HOST_REBOOT_RESULT_FILE}"
systemctl stop omt-client-host-debug.path omt-client-host-debug.timer \
    omt-client-host-debug.service 2>/dev/null || true
systemctl disable omt-client-host-debug.path omt-client-host-debug.timer \
    omt-client-host-debug.service 2>/dev/null || true
rm -f "${LEGACY_HOST_DEBUG_SERVICE_FILE}" "${LEGACY_HOST_DEBUG_PATH_FILE}" \
    "${LEGACY_HOST_DEBUG_TIMER_FILE}"

host_publish_systemd_unit "${AVAHI_PROXY_SERVICE_FILE}" <<EOF
[Unit]
Description=Filtered Avahi system bus proxy for OMT Client
Requires=dbus.service avahi-daemon.service
After=dbus.service avahi-daemon.service
StartLimitIntervalSec=0

[Service]
Type=simple
User=root
Group=${OMT_GID}
UMask=0117
ExecStartPre=/usr/bin/rm -f ${AVAHI_PROXY_SOCKET}
ExecStart=/usr/bin/xdg-dbus-proxy unix:path=/run/dbus/system_bus_socket ${AVAHI_PROXY_SOCKET} --filter --talk=org.freedesktop.Avahi
ExecStartPost=/bin/bash -c 'for attempt in {1..50}; do if [ -S ${AVAHI_PROXY_SOCKET} ]; then chown root:${OMT_GID} ${AVAHI_PROXY_SOCKET}; chmod 0660 ${AVAHI_PROXY_SOCKET}; exit 0; fi; sleep 0.1; done; exit 1'
ExecStopPost=/usr/bin/rm -f ${AVAHI_PROXY_SOCKET}
Restart=always
RestartSec=1
NoNewPrivileges=true
PrivateTmp=true
ProtectHome=true
ProtectSystem=strict
ProtectKernelTunables=true
ProtectControlGroups=true
ReadWritePaths=${AVAHI_STATE_DIR}

[Install]
WantedBy=multi-user.target
EOF

host_publish_systemd_unit "${HOST_DIAGNOSTICS_SERVICE_FILE}" <<EOF
[Unit]
Description=Collect OMT Client host diagnostics
Wants=docker.service dbus.service avahi-daemon.service network-online.target omt-client-avahi-proxy.service
After=docker.service dbus.service avahi-daemon.service network-online.target omt-client-avahi-proxy.service

[Service]
Type=oneshot
Environment=OMT_INSTALL_DIR=${INSTALL_DIR}
Environment=OMT_DIAGNOSTICS_HOST_BUDGET_SECONDS=25
Environment=OMT_DIAGNOSTICS_HOST_REPORT_FILE=${HOST_DIAGNOSTICS_REPORT_FILE}
Environment=OMT_DIAGNOSTICS_HOST_REQUEST_FILE=${HOST_DIAGNOSTICS_REQUEST_FILE}
ExecStart=${HOST_DIAGNOSTICS_INSTALLED_SCRIPT}
UMask=0027
NoNewPrivileges=true
PrivateTmp=true
ProtectHome=read-only
ProtectSystem=strict
ProtectKernelTunables=true
ProtectControlGroups=true
ReadWritePaths=${HOST_DIAGNOSTICS_STATE_DIR}
EOF

host_publish_systemd_unit "${HOST_DIAGNOSTICS_PATH_FILE}" <<EOF
[Unit]
Description=Run OMT Client host diagnostics on web request

[Path]
PathChanged=${HOST_DIAGNOSTICS_REQUEST_FILE}
Unit=omt-client-host-diagnostics.service

[Install]
WantedBy=multi-user.target
EOF

host_publish_systemd_unit "${HOST_REBOOT_SERVICE_FILE}" <<EOF
[Unit]
Description=Validate an OMT Client host reboot request
After=omt-client.service

[Service]
Type=oneshot
Environment=OMT_UID=${OMT_UID}
Environment=OMT_GID=${OMT_GID}
ExecStart=${HOST_REBOOT_INSTALLED_SCRIPT}
TimeoutStartSec=15
UMask=0027
NoNewPrivileges=true
PrivateDevices=true
PrivateTmp=true
ProtectClock=true
ProtectControlGroups=true
ProtectHome=true
ProtectHostname=true
ProtectKernelLogs=true
ProtectKernelModules=true
ProtectKernelTunables=true
ProtectSystem=strict
RestrictAddressFamilies=AF_UNIX
ReadWritePaths=${HOST_ACTION_STATE_DIR} /run/lock
CapabilityBoundingSet=CAP_SYS_BOOT
AmbientCapabilities=CAP_SYS_BOOT
EOF

host_publish_systemd_unit "${HOST_REBOOT_PATH_FILE}" <<EOF
[Unit]
Description=Watch for validated OMT Client reboot requests

[Path]
PathChanged=${HOST_REBOOT_REQUEST_FILE}
Unit=omt-client-reboot.service

[Install]
WantedBy=multi-user.target
EOF

# ─── Configure host firewall from the image's web port ──────────────────────

if command -v firewall-cmd >/dev/null 2>&1 && systemctl is-active --quiet firewalld; then
    echo "Configuring firewalld for mDNS discovery and web access..."
    firewall-cmd --permanent --add-service=mdns || true
    firewall-cmd --permanent --add-port="${WEB_PORT}/tcp" || true
    firewall-cmd --reload || true
    firewall-cmd --add-service=mdns || true
    firewall-cmd --add-port="${WEB_PORT}/tcp" || true
fi

UFW_STATUS=""
if command -v ufw >/dev/null 2>&1; then
    UFW_STATUS="$(ufw status 2>/dev/null || true)"
fi
if grep -qi '^Status: active' <<< "${UFW_STATUS}"; then
    echo "Configuring ufw for mDNS discovery and web access..."
    ufw allow 5353/udp comment 'OMT mDNS discovery' || true
    ufw allow "${WEB_PORT}/tcp" comment 'OMT web UI' || true
fi

# ─── Configure KMS/DRM and optional connector forcing ───────────────────────

CONFIG_FILE="/boot/firmware/config.txt"
if [[ -f "${CONFIG_FILE}" ]]; then
    echo "Configuring HDMI output..."
    HDMI_TMP="$(mktemp "${CONFIG_FILE}.omt-client.XXXXXX")"
    host_hdmi_config_txt < "${CONFIG_FILE}" > "${HDMI_TMP}"
    chmod --reference="${CONFIG_FILE}" "${HDMI_TMP}"
    chown --reference="${CONFIG_FILE}" "${HDMI_TMP}"
    sync -f "${HDMI_TMP}"
    mv -fT "${HDMI_TMP}" "${CONFIG_FILE}"
    sync -d "$(dirname -- "${CONFIG_FILE}")"
fi

PREVIOUS_HDMI_TOKEN=""
if [[ "${SAVED_HDMI_VIDEO_MODE}" != "auto" ]]; then
    PREVIOUS_HDMI_TOKEN="video=${SAVED_HDMI_VIDEO_MODE}D"
fi
DESIRED_HDMI_TOKEN=""
DESIRED_HDMI_CONNECTOR=""
if [[ "${HDMI_VIDEO_MODE}" != "auto" ]]; then
    DESIRED_HDMI_TOKEN="video=${HDMI_VIDEO_MODE}D"
    DESIRED_HDMI_CONNECTOR="${HDMI_VIDEO_MODE%%:*}"
fi

if [[ -f "${CMDLINE_FILE}" ]]; then
    mapfile -t cmdline_lines < "${CMDLINE_FILE}"
    if [[ "${#cmdline_lines[@]}" -ne 1 || -z "${cmdline_lines[0]}" ]]; then
        echo "ERROR: ${CMDLINE_FILE} must contain exactly one non-empty line." >&2
        exit 1
    fi
    if ! UPDATED_CMDLINE="$(host_hdmi_cmdline_line "${cmdline_lines[0]}" \
            "${PREVIOUS_HDMI_TOKEN}" "${DESIRED_HDMI_TOKEN}" "${DESIRED_HDMI_CONNECTOR}")"; then
        echo "ERROR: ${CMDLINE_FILE} already contains an unmanaged video setting." >&2
        exit 1
    fi
    if [[ "${UPDATED_CMDLINE}" != "${cmdline_lines[0]}" ]]; then
        CMDLINE_TMP="$(mktemp "${CMDLINE_FILE}.omt-client.XXXXXX")"
        printf '%s\n' "${UPDATED_CMDLINE}" > "${CMDLINE_TMP}"
        chmod --reference="${CMDLINE_FILE}" "${CMDLINE_TMP}"
        chown --reference="${CMDLINE_FILE}" "${CMDLINE_TMP}"
        sync -f "${CMDLINE_TMP}"
        mv -fT "${CMDLINE_TMP}" "${CMDLINE_FILE}"
        sync -d "$(dirname -- "${CMDLINE_FILE}")"
    fi
elif [[ "${HDMI_VIDEO_MODE}" != "auto" ]]; then
    echo "ERROR: ${CMDLINE_FILE} is required for forced KMS HDMI output." >&2
    exit 1
fi

install -d -m 0755 "${INSTALLER_CONFIG_DIR}"
INSTALLER_CONFIG_TMP="$(mktemp "${INSTALLER_CONFIG_FILE}.tmp.XXXXXX")"
printf 'HDMI_VIDEO_MODE=%s\n' "${HDMI_VIDEO_MODE}" > "${INSTALLER_CONFIG_TMP}"
chmod 0644 "${INSTALLER_CONFIG_TMP}"
chown root:root "${INSTALLER_CONFIG_TMP}"
sync -f "${INSTALLER_CONFIG_TMP}"
mv -fT "${INSTALLER_CONFIG_TMP}" "${INSTALLER_CONFIG_FILE}"
sync -d "${INSTALLER_CONFIG_DIR}"

# ─── Install boot service and start when runtime devices are ready ──────────

host_publish_systemd_unit /etc/systemd/system/omt-client.service <<EOF
[Unit]
Description=OMT Client Docker Container
Requires=docker.service
Wants=dbus.service avahi-daemon.service network-online.target omt-client-avahi-proxy.service
After=docker.service dbus.service avahi-daemon.service network-online.target omt-client-avahi-proxy.service

[Service]
Type=oneshot
RemainAfterExit=yes
WorkingDirectory=${INSTALL_DIR}
ExecStartPre=${DEPLOY_RECOVERY_HELPER} recover ${INSTALL_DIR} ${DEPLOY_RECOVERY_MANIFEST}
ExecStartPre=/bin/bash -c 'find /dev/dri -maxdepth 1 -type c -name "card*" -print -quit 2>/dev/null | grep -q . || { echo "OMT Client requires a primary DRM card under /dev/dri" >&2; exit 1; }'
ExecStartPre=/bin/bash -c 'find /dev/snd -maxdepth 1 -type c -name "pcmC*D*p" -print -quit 2>/dev/null | grep -q . || { echo "OMT Client requires an ALSA playback PCM under /dev/snd" >&2; exit 1; }'
ExecStart=/usr/bin/docker compose --env-file ${COMPOSE_ENV_FILE} -f ${COMPOSE_FILE} up -d
ExecStop=/usr/bin/docker compose --env-file ${COMPOSE_ENV_FILE} -f ${COMPOSE_FILE} down
TimeoutStartSec=0

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable omt-client.service
systemctl enable omt-client-avahi-proxy.service
if ! systemctl restart omt-client-avahi-proxy.service; then
    echo "WARNING: The filtered Avahi proxy is unavailable; discovery will recover when its service restarts."
fi
systemctl enable --now omt-client-host-diagnostics.path
systemctl enable --now omt-client-reboot.path

STARTUP_DEFERRED=false
if [[ "${RUNTIME_DEVICES_READY}" == "true" ]]; then
    echo "Starting container through systemd..."
    systemctl restart omt-client.service
else
    STARTUP_DEFERRED=true
    MISSING_RUNTIME_DEVICE_LIST="${MISSING_RUNTIME_DEVICES[0]}"
    for missing_runtime_device in "${MISSING_RUNTIME_DEVICES[@]:1}"; do
        MISSING_RUNTIME_DEVICE_LIST+=", ${missing_runtime_device}"
    done
    echo "Container startup deferred; missing runtime devices: ${MISSING_RUNTIME_DEVICE_LIST}."
    echo "The enabled omt-client service will start after reboot when those devices are ready."
fi

# The v2 nested capsule is now installed and its recovery helper is durable.
# Remove only release-v1 names that are known to have lived at the install root.
rm -f -- \
    "${INSTALL_DIR}/docker-compose.yml" \
    "${INSTALL_DIR}/install.sh" \
    "${INSTALL_DIR}/uninstall.sh" \
    "${INSTALL_DIR}/host-debug.sh" \
    "${INSTALL_DIR}/host-reboot.sh" \
    "${INSTALL_DIR}/deploy-transaction.sh" \
    "${INSTALL_DIR}/deploy-artifacts.txt" \
    "${HOST_COMPONENT_DIR}/host-debug.sh" \
    "${HOST_COMPONENT_DIR}/deploy-artifacts.txt"

# ─── Print authoritative summary ─────────────────────────────────────────────

HOSTNAME_I="$(hostname -I || true)"
IP_ADDR="$(awk '{print $1}' <<< "${HOSTNAME_I}")"

echo ""
echo "=== Installation Complete ==="
echo ""
echo "Install:  ${INSTALL_DIR}"
if [[ "${STARTUP_DEFERRED}" == "true" ]]; then
    echo "Web UI:   https://${IP_ADDR}:${WEB_PORT} (available after service startup)"
else
    echo "Web UI:   https://${IP_ADDR}:${WEB_PORT}"
fi
echo "          (self-signed cert — accept the browser warning on first visit)"
echo "HDMI:     ${HDMI_VIDEO_MODE}"
echo ""
echo "Password: after the service starts, check container logs for the auto-generated password:"
echo "          docker compose -f ${COMPOSE_FILE} logs | grep -A 1 'Web UI password'"
echo ""
echo "Container status: docker compose -f ${COMPOSE_FILE} ps"
echo "View logs:        docker compose -f ${COMPOSE_FILE} logs -f"
echo ""
if [[ "${STARTUP_DEFERRED}" == "true" ]]; then
    echo "A reboot is required to apply device configuration and start the service."
else
    echo "A reboot is recommended to apply HDMI configuration."
fi
read -r -p "Reboot now? (y/N): " REBOOT_CHOICE
if [[ "${REBOOT_CHOICE}" =~ ^[Yy] ]]; then
    reboot
fi
