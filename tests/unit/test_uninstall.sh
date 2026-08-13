#!/bin/bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
UNINSTALL="${ROOT}/deploy/host/uninstall.sh"
bash -n "${UNINSTALL}"

require() {
    grep -Eq -- "$1" "${UNINSTALL}" || {
        echo "FAIL: $2" >&2
        exit 1
    }
}
forbid() {
    ! grep -Eq -- "$1" "${UNINSTALL}" || {
        echo "FAIL: $2" >&2
        exit 1
    }
}

require 'docker compose.*down --remove-orphans' "Compose workload must be stopped"
require 'docker rmi omt-client' "OMT image must be removed"
require 'rc-service.*stop' "OpenRC services must be stopped"
require 'rc-update del.*default' "OpenRC services must be disabled"
require 'host_remove_openrc_services' "owned OpenRC scripts must be removed"
require 'host-event-watcher\.sh' "root-owned event bridge must be removed"
require 'HOST_STATE_DIR="/var/lib/omt-client"' "host state target must be fixed"
require 'rm -rf "\$\{HOST_STATE_DIR\}"' "OMT host state must be removed"
require 'docker volume rm omt-config-v3' "the OMT volume must be removed"
forbid 'systemctl' "Alpine uninstaller must not call systemd"
require 'read -r -p "Remove \$\{INSTALL_DIR\} and all OMT Client volume data\?' \
    "persistent-state removal must require confirmation"
require 'Raspberry Pi Alpine OMT Client Uninstaller' "uninstaller branding must not be Pi 5 only"
require 'omt-client-cpufreq.start' "the CPU governor drop-in must be removed"
require 'omt-client-blacklist\.conf' "the Bluetooth module blacklist must be removed"
require 'omt-client-action\.sh' "the Wi-Fi power-save action hook must be removed"

echo "Alpine OMT uninstaller contract tests passed"
