#!/bin/bash
# Static checks for production docker-compose.yml invariants.
#
# Run: ./tests/unit/test_compose_config.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
COMPOSE_FILE="${PROJECT_ROOT}/docker-compose.yml"

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

PASS=0
FAIL=0

pass() { echo -e "${GREEN}PASS${NC}: $1"; PASS=$((PASS + 1)); }
fail() { echo -e "${RED}FAIL${NC}: $1"; FAIL=$((FAIL + 1)); }

echo "=== docker-compose.yml Config Tests ==="

assert_contains() {
    local pattern="$1"
    local label="$2"
    if grep -Eq -- "${pattern}" "${COMPOSE_FILE}"; then
        pass "${label}"
    else
        fail "${label}"
    fi
}

assert_not_contains() {
    local pattern="$1"
    local label="$2"
    if grep -Eq -- "${pattern}" "${COMPOSE_FILE}"; then
        fail "${label}"
    else
        pass "${label}"
    fi
}

assert_contains '^[[:space:]]*restart:[[:space:]]+unless-stopped[[:space:]]*$' \
    "production container restarts unless stopped"
assert_contains '^[[:space:]]*network_mode:[[:space:]]+host[[:space:]]*$' \
    "production container uses host networking"
assert_contains '^[[:space:]]*read_only:[[:space:]]+true[[:space:]]*$' \
    "production container uses a read-only root filesystem"
assert_contains '^[[:space:]]*-[[:space:]]+no-new-privileges:true[[:space:]]*$' \
    "production container prevents privilege escalation"
assert_contains '^[[:space:]]*-[[:space:]]+systempaths=unconfined[[:space:]]*$' \
    "production container exposes proc ALSA state"
assert_contains '^[[:space:]]*-[[:space:]]+ALL[[:space:]]*$' \
    "production container drops Linux capabilities"
assert_contains '^[[:space:]]*-[[:space:]]+/dev/dri:/dev/dri[[:space:]]*$' \
    "DRM device is passed through"
assert_contains '^[[:space:]]*-[[:space:]]+/dev/snd:/dev/snd[[:space:]]*$' \
    "ALSA device is passed through"
assert_contains '^[[:space:]]*-[[:space:]]+omt-config:/etc/omt[[:space:]]*$' \
    "persistent config volume is mounted at /etc/omt"
assert_contains '^[[:space:]]*-[[:space:]]+/var/lib/omt-client:/host-debug[[:space:]]*$' \
    "host diagnostics request directory is mounted"
assert_contains '^[[:space:]]*-[[:space:]]+/var/lib/omt-client/host-actions:/host-actions[[:space:]]*$' \
    "host reboot action directory is mounted"
assert_not_contains '/run/dbus/system_bus_socket' \
    "raw host system D-Bus socket is not exposed to the container"
assert_contains '^[[:space:]]+DBUS_SYSTEM_BUS_ADDRESS:[[:space:]]+unix:path=/host-debug/avahi-system-bus[[:space:]]*$' \
    "container discovery points at the filtered Avahi proxy"
assert_contains '^[[:space:]]+OMT_HDMI_CONNECTOR:[[:space:]]+"\$\{OMT_HDMI_CONNECTOR:-auto\}"[[:space:]]*$' \
    "installer-managed HDMI connector policy is propagated"
assert_contains '^[[:space:]]+OMT_REBOOT_REQUEST_FILE:[[:space:]]+/host-actions/reboot.request[[:space:]]*$' \
    "Web reboot request uses the fixed action file"
assert_contains '^[[:space:]]+OMT_REBOOT_RESULT_FILE:[[:space:]]+/host-actions/reboot.result[[:space:]]*$' \
    "Web reboot acknowledgement uses the fixed result file"
assert_not_contains '^[[:space:]]*-[[:space:]]+/proc(/|:)' \
    "proc subpaths are not bind-mounted because current runc rejects them"
assert_contains '^[[:space:]]*-[[:space:]]+/tmp[[:space:]]*$' \
    "tmpfs is mounted at /tmp"
assert_contains '^[[:space:]]*-[[:space:]]+"\$\{OMT_VIDEO_GID:-44\}"[[:space:]]*(#.*)?$' \
    "video group uses an installer-provided GID with a safe fallback"
assert_contains '^[[:space:]]*-[[:space:]]+"\$\{OMT_RENDER_GID:-106\}"[[:space:]]*(#.*)?$' \
    "render group uses an installer-provided GID with a safe fallback"
assert_contains '^[[:space:]]*-[[:space:]]+"\$\{OMT_AUDIO_GID:-29\}"[[:space:]]*(#.*)?$' \
    "audio group uses an installer-provided GID with a safe fallback"
assert_contains '^volumes:[[:space:]]*$' \
    "top-level volumes section exists"
assert_contains '^[[:space:]]+omt-config:[[:space:]]*$' \
    "omt-config volume is declared"
assert_contains '^[[:space:]]+name:[[:space:]]+omt-config[[:space:]]*$' \
    "omt-config has a stable platform-wide name"
assert_contains '^[[:space:]]+external:[[:space:]]+true[[:space:]]*$' \
    "installer-created omt-config volume is declared external"

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[[ "${FAIL}" -eq 0 ]]
