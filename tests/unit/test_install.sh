#!/bin/bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
INSTALL="${ROOT}/deploy/host/install.sh"

bash -n "${INSTALL}"

require() {
    local pattern="$1" message="$2"
    if ! grep -Eq -- "${pattern}" "${INSTALL}"; then
        echo "FAIL: ${message}" >&2
        exit 1
    fi
}
forbid() {
    local pattern="$1" message="$2"
    if grep -Eq -- "${pattern}" "${INSTALL}"; then
        echo "FAIL: ${message}" >&2
        exit 1
    fi
}

require 'ARCH.*!=.*aarch64' "installer must reject non-ARM64 hosts"
require 'HOST_REBOOT_SCRIPT=.*host-reboot\.sh' "reboot helper must be required"
require 'PROJECT_LICENSE=.*LICENSE' "project license must be required"
require 'THIRD_PARTY_NOTICES=.*THIRD_PARTY_NOTICES\.txt' "notices must be required"
require 'STABLE_VOLUME="omt-config"' "stable OMT volume name must be explicit"
forbid 'LEGACY_VOLUME=' "OMT installer must not define a legacy migration volume"
forbid 'Migrating legacy' "OMT installer must not migrate legacy state"
require '/etc/systemd/system/ndi-client\.service' "legacy NDI service must be detected"
require 'docker container inspect ndi-client' "legacy NDI container must be detected"
require 'does not migrate or modify it' "clean-break failure must explain policy"

legacy_line="$(grep -n 'legacy_paths=(' "${INSTALL}" | cut -d: -f1)"
mutation_line="$(grep -n 'systemctl set-default multi-user.target' "${INSTALL}" | cut -d: -f1)"
(( legacy_line < mutation_line )) || {
    echo "FAIL: legacy preflight must run before host mutation" >&2
    exit 1
}

require 'source_target\.json omt/settings\.xml' "new OMT state files must be permissioned"
require 'HOST_ACTION_STATE_DIR=.*host-actions' "host action directory must be isolated"
require 'chmod 0600.*HOST_REBOOT_REQUEST_FILE' "request file must be mode 0600"
require 'chmod 0640.*HOST_REBOOT_RESULT_FILE' "result file must be mode 0640"
require 'Environment=OMT_UID=' "reboot validator must receive fixed image UID"
require 'Environment=OMT_GID=' "reboot validator must receive fixed image GID"
require 'CapabilityBoundingSet=CAP_SYS_BOOT' "reboot unit must bound capabilities"
require 'ProtectSystem=strict' "reboot unit must protect the host filesystem"
require 'RestrictAddressFamilies=AF_UNIX' "reboot unit must restrict networking"
require 'PathChanged=\$\{HOST_REBOOT_REQUEST_FILE\}' "reboot path unit must watch the fixed request"
require 'systemctl enable --now omt-client-reboot\.path' "reboot watcher must be enabled"

forbid '5960-8999' "installer must not open a broad legacy media port range"
require 'add-service=mdns' "firewalld must allow mDNS"
require 'ufw allow 5353/udp' "ufw must allow mDNS"

echo "OMT installer contract tests passed"
