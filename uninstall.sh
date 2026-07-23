#!/bin/bash
# RPi OMT Client Uninstaller
# Usage: sudo /absolute/path/to/omt-client/uninstall.sh

set -euo pipefail

export LC_ALL=C

validate_install_dir() {
    local path="$1"
    [[ "${path}" != "/" ]] || return 1
    [[ "${path}" =~ ^/[A-Za-z0-9._/-]+$ ]] || return 1
    [[ "${path}" != *"//"* ]] || return 1
    [[ "${path}" != */./* && "${path}" != */../* ]] || return 1
    [[ "${path}" != */. && "${path}" != */.. ]] || return 1
}

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
if ! validate_install_dir "${SCRIPT_DIR}"; then
    echo "ERROR: Invalid install directory: ${SCRIPT_DIR}" >&2
    exit 1
fi

INSTALL_DIR="${SCRIPT_DIR}"
COMPOSE_FILE="${INSTALL_DIR}/docker-compose.yml"
COMPOSE_ENV_FILE="${INSTALL_DIR}/.env"
HOST_DEBUG_SERVICE_FILE="/etc/systemd/system/omt-client-host-debug.service"
HOST_DEBUG_PATH_FILE="/etc/systemd/system/omt-client-host-debug.path"
LEGACY_HOST_DEBUG_TIMER_FILE="/etc/systemd/system/omt-client-host-debug.timer"
HOST_DEBUG_STATE_DIR="/var/lib/omt-client"
AVAHI_PROXY_SERVICE_FILE="/etc/systemd/system/omt-client-avahi-proxy.service"
AVAHI_PROXY_SOCKET="${HOST_DEBUG_STATE_DIR}/avahi-system-bus"
HOST_DEBUG_INSTALLED_DIR="/usr/local/libexec/omt-client"
HOST_DEBUG_INSTALLED_SCRIPT="${HOST_DEBUG_INSTALLED_DIR}/host-debug.sh"
HOST_REBOOT_INSTALLED_SCRIPT="${HOST_DEBUG_INSTALLED_DIR}/host-reboot.sh"
DEPLOY_RECOVERY_HELPER="${HOST_DEBUG_INSTALLED_DIR}/recover-deployment.sh"
DEPLOY_RECOVERY_MANIFEST="${HOST_DEBUG_INSTALLED_DIR}/deploy-artifacts.txt"
HOST_REBOOT_SERVICE_FILE="/etc/systemd/system/omt-client-reboot.service"
HOST_REBOOT_PATH_FILE="/etc/systemd/system/omt-client-reboot.path"

echo "=== RPi OMT Client Uninstaller ==="

if [[ "${EUID}" -ne 0 ]]; then
    echo "ERROR: Please run as root (sudo ${INSTALL_DIR}/uninstall.sh)"
    exit 1
fi

if command -v docker >/dev/null 2>&1 && [[ -f "${COMPOSE_FILE}" ]]; then
    echo "Stopping container..."
    compose_args=(-f "${COMPOSE_FILE}")
    if [[ -f "${COMPOSE_ENV_FILE}" ]]; then
        compose_args=(--env-file "${COMPOSE_ENV_FILE}" "${compose_args[@]}")
    fi
    docker compose "${compose_args[@]}" down --remove-orphans 2>/dev/null || true
fi

if command -v docker >/dev/null 2>&1; then
    docker rmi omt-client 2>/dev/null || true
fi

if [[ -f /etc/systemd/system/omt-client.service ]]; then
    echo "Removing systemd service..."
    systemctl stop omt-client.service 2>/dev/null || true
    systemctl disable omt-client.service 2>/dev/null || true
    rm -f /etc/systemd/system/omt-client.service
fi

systemctl stop omt-client-host-debug.path omt-client-host-debug.timer \
    omt-client-host-debug.service 2>/dev/null || true
systemctl disable omt-client-host-debug.path omt-client-host-debug.timer \
    omt-client-host-debug.service 2>/dev/null || true
rm -f "${HOST_DEBUG_PATH_FILE}" "${LEGACY_HOST_DEBUG_TIMER_FILE}" \
    "${HOST_DEBUG_SERVICE_FILE}"
systemctl stop omt-client-reboot.path omt-client-reboot.service 2>/dev/null || true
systemctl disable omt-client-reboot.path 2>/dev/null || true
rm -f "${HOST_REBOOT_PATH_FILE}" "${HOST_REBOOT_SERVICE_FILE}"
systemctl stop omt-client-avahi-proxy.service 2>/dev/null || true
systemctl disable omt-client-avahi-proxy.service 2>/dev/null || true
rm -f "${AVAHI_PROXY_SOCKET}" "${AVAHI_PROXY_SERVICE_FILE}"
rm -rf "${HOST_DEBUG_STATE_DIR}"
rm -f "${HOST_DEBUG_INSTALLED_SCRIPT}" "${HOST_REBOOT_INSTALLED_SCRIPT}" \
    "${DEPLOY_RECOVERY_HELPER}" \
    "${DEPLOY_RECOVERY_MANIFEST}"
rmdir "${HOST_DEBUG_INSTALLED_DIR}" 2>/dev/null || true
systemctl daemon-reload

read -r -p "Remove ${INSTALL_DIR} and all OMT Client volume data? (y/N): " REMOVE_DIR
if [[ "${REMOVE_DIR}" =~ ^[Yy] ]]; then
    if ! command -v docker >/dev/null 2>&1 || ! docker info >/dev/null 2>&1; then
        echo "ERROR: Docker is required to verify removal of OMT Client volume data." >&2
        echo "The install directory was retained so uninstallation can be retried safely." >&2
        exit 1
    fi

    if docker volume inspect omt-config >/dev/null 2>&1 && \
       ! docker volume rm omt-config >/dev/null; then
        echo "ERROR: Could not remove Docker volume omt-config." >&2
        echo "The install directory was retained so uninstallation can be retried safely." >&2
        exit 1
    fi

    rm -rf "${INSTALL_DIR}"
    echo "Removed ${INSTALL_DIR} and omt-config."
fi

echo ""
echo "=== Uninstall Complete ==="
