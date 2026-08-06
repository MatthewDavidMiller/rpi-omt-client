#!/bin/bash
# Raspberry Pi 5 Alpine OMT Client uninstaller.

set -euo pipefail
export LC_ALL=C

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
INSTALL_DIR="$(cd -- "${SCRIPT_DIR}/../.." && pwd -P)"
# shellcheck source=deploy/lib/host-validation.sh
source "${INSTALL_DIR}/deploy/lib/host-validation.sh"
# shellcheck source=deploy/lib/service-install.sh
source "${INSTALL_DIR}/deploy/lib/service-install.sh"
host_validate_safe_absolute_path "${INSTALL_DIR}" || {
    echo "ERROR: Invalid install directory: ${INSTALL_DIR}" >&2
    exit 1
}
[[ "${EUID}" -eq 0 ]] || {
    echo "ERROR: Run this uninstaller as root through sudo." >&2
    exit 1
}
[[ -r /etc/alpine-release ]] || {
    echo "ERROR: This uninstaller supports only Alpine Linux." >&2
    exit 1
}

COMPOSE_FILE="${INSTALL_DIR}/deploy/compose.yml"
COMPOSE_ENV_FILE="${INSTALL_DIR}/deploy/.env"
HOST_STATE_DIR="/var/lib/omt-client"
HOST_COMPONENT_DIR="/usr/local/libexec/omt-client"
OPENRC_SERVICES=(
    omt-client
    omt-client-avahi-proxy
    omt-client-host-diagnostics
    omt-client-reboot
)

echo "=== Raspberry Pi 5 Alpine OMT Client Uninstaller ==="

for service in "${OPENRC_SERVICES[@]}"; do
    rc-service "${service}" stop >/dev/null 2>&1 || true
    rc-update del "${service}" default >/dev/null 2>&1 || true
done

if command -v docker >/dev/null 2>&1 && [[ -f "${COMPOSE_FILE}" ]]; then
    compose_args=(-f "${COMPOSE_FILE}")
    [[ ! -f "${COMPOSE_ENV_FILE}" ]] || \
        compose_args=(--env-file "${COMPOSE_ENV_FILE}" "${compose_args[@]}")
    docker compose "${compose_args[@]}" down --remove-orphans 2>/dev/null || true
    docker rmi omt-client 2>/dev/null || true
fi

host_remove_openrc_services "${OPENRC_SERVICES[@]}"
for service in "${OPENRC_SERVICES[@]}"; do
    rm -f -- "/etc/conf.d/${service}"
done
rm -f -- /etc/nftables.d/omt-client.nft \
    /etc/ssh/sshd_config.d/90-omt-client-hardening.conf \
    /etc/sysctl.d/90-omt-client-hardening.conf
if command -v nft >/dev/null 2>&1 && [[ -f /etc/nftables.nft ]]; then
    nft -f /etc/nftables.nft || true
fi
if command -v sysctl >/dev/null 2>&1; then
    sysctl --system >/dev/null || true
fi
if command -v sshd >/dev/null 2>&1 && sshd -t && \
   rc-service sshd status >/dev/null 2>&1; then
    rc-service sshd reload || true
fi

rm -rf "${HOST_STATE_DIR}"
rm -f -- \
    "${HOST_COMPONENT_DIR}/host-diagnostics.sh" \
    "${HOST_COMPONENT_DIR}/host-event-watcher.sh" \
    "${HOST_COMPONENT_DIR}/host-reboot.sh" \
    "${HOST_COMPONENT_DIR}/reboot-request.sh" \
    "${HOST_COMPONENT_DIR}/recover-deployment.sh" \
    "${HOST_COMPONENT_DIR}/manifest-v3.txt" \
    "${HOST_COMPONENT_DIR}/deploy-artifacts.txt"
rmdir "${HOST_COMPONENT_DIR}" 2>/dev/null || true

read -r -p "Remove ${INSTALL_DIR} and all OMT Client volume data? (y/N): " REMOVE_DIR
if [[ "${REMOVE_DIR}" =~ ^[Yy] ]]; then
    if ! command -v docker >/dev/null 2>&1 || ! docker info >/dev/null 2>&1; then
        echo "ERROR: Docker is required to verify OMT volume removal." >&2
        exit 1
    fi
    if docker volume inspect omt-config-v3 >/dev/null 2>&1 && \
       ! docker volume rm omt-config-v3 >/dev/null; then
        echo "ERROR: Could not remove Docker volume omt-config-v3." >&2
        exit 1
    fi
    rm -rf "${INSTALL_DIR}"
    echo "Removed ${INSTALL_DIR} and omt-config-v3."
fi

echo "Docker log policy, zram, and Wi-Fi configuration were retained as safe host defaults."
echo "=== Uninstall Complete ==="
