#!/bin/bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
UNINSTALL="${ROOT}/uninstall.sh"

bash -n "${UNINSTALL}"

require() {
    local pattern="$1" message="$2"
    grep -Eq -- "${pattern}" "${UNINSTALL}" || {
        echo "FAIL: ${message}" >&2
        exit 1
    }
}
forbid() {
    local pattern="$1" message="$2"
    if grep -Eq -- "${pattern}" "${UNINSTALL}"; then
        echo "FAIL: ${message}" >&2
        exit 1
    fi
}

require 'docker compose.*down --remove-orphans' "Compose workload must be stopped"
require 'docker rmi omt-client' "OMT image must be removed"
require 'omt-client-reboot\.path omt-client-reboot\.service' "reboot units must be stopped"
require 'HOST_REBOOT_INSTALLED_SCRIPT' "root-owned reboot helper must be removed"
require 'HOST_DEBUG_STATE_DIR="/var/lib/omt-client"' "host state target must be fixed"
require 'rm -rf "\$\{HOST_DEBUG_STATE_DIR\}"' "OMT host state must be removed"
require 'docker volume rm omt-config' "only the OMT volume may be removed"
forbid 'omt-client_omt-config' "uninstaller must not touch a legacy/migration volume"
require 'read -r -p "Remove \$\{INSTALL_DIR\} and all OMT Client volume data\?' \
    "destructive persistent-state removal must require confirmation"
require 'rm -rf "\$\{INSTALL_DIR\}"' "confirmed install directory must be removed"

echo "OMT uninstaller contract tests passed"
