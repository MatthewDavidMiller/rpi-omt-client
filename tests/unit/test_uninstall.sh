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
require 'docker volume rm omt-config-v3' "only the native-generation OMT volume may be removed"
forbid 'docker volume rm omt-config ' "legacy OMT volume must remain untouched"
forbid 'systemctl' "Alpine uninstaller must not call systemd"
require 'read -r -p "Remove \$\{INSTALL_DIR\} and all OMT Client volume data\?' \
    "persistent-state removal must require confirmation"
require 'rm -rf "\$\{INSTALL_DIR\}"' "confirmed install directory must be removed"

echo "Alpine OMT uninstaller contract tests passed"
