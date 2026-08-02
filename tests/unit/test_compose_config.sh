#!/bin/bash
# Static checks for production deploy/compose.yml invariants.
#
# Run: ./tests/unit/test_compose_config.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
COMPOSE_FILE="${PROJECT_ROOT}/deploy/compose.yml"

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

PASS=0
FAIL=0

pass() { echo -e "${GREEN}PASS${NC}: $1"; PASS=$((PASS + 1)); }
fail() { echo -e "${RED}FAIL${NC}: $1"; FAIL=$((FAIL + 1)); }

echo "=== deploy/compose.yml Config Tests ==="

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
assert_contains '^[[:space:]]*-[[:space:]]+/var/lib/omt-client/diagnostics:/host-diagnostics[[:space:]]*$' \
    "least-privilege host diagnostics directory is mounted"
assert_contains '^[[:space:]]*-[[:space:]]+/var/lib/omt-client/avahi:/host-avahi:ro[[:space:]]*$' \
    "least-privilege Avahi proxy directory is mounted read-only"
assert_contains '^[[:space:]]*-[[:space:]]+/var/lib/omt-client/host-actions:/host-actions[[:space:]]*$' \
    "host reboot action directory is mounted"
assert_not_contains '/run/dbus/system_bus_socket' \
    "raw host system D-Bus socket is not exposed to the container"
assert_contains '^[[:space:]]+DBUS_SYSTEM_BUS_ADDRESS:[[:space:]]+unix:path=/host-avahi/system-bus[[:space:]]*$' \
    "container discovery points at the filtered Avahi proxy"
assert_contains '^[[:space:]]+OMT_HDMI_CONNECTOR:[[:space:]]+"\$\{OMT_HDMI_CONNECTOR:-auto\}"[[:space:]]*$' \
    "installer-managed HDMI connector policy is propagated"
assert_contains '^[[:space:]]+OMT_REBOOT_REQUEST_FILE:[[:space:]]+/host-actions/reboot.request[[:space:]]*$' \
    "Web reboot request uses the fixed action file"
assert_contains '^[[:space:]]+OMT_REBOOT_RESULT_FILE:[[:space:]]+/host-actions/reboot.result[[:space:]]*$' \
    "Web reboot acknowledgement uses the fixed result file"
assert_not_contains '^[[:space:]]*-[[:space:]]+/proc(/|:)' \
    "proc subpaths are not bind-mounted because current runc rejects them"
assert_contains '^[[:space:]]*target:[[:space:]]+/tmp[[:space:]]*$' \
    "tmpfs is mounted at /tmp"
assert_contains '^[[:space:]]*mem_limit:[[:space:]]+"\$\{OMT_CONTAINER_MEMORY_LIMIT:-768m\}"[[:space:]]*$' \
    "container memory is bounded for low-RAM Pi 5 models"
assert_contains '^[[:space:]]*pids_limit:[[:space:]]+128[[:space:]]*$' \
    "container process count is bounded"
assert_contains '^[[:space:]]*driver:[[:space:]]+local[[:space:]]*$' \
    "container logs use Docker's bounded local driver"
# Playback status is rewritten continuously, so a runtime directory left on the
# omt-config volume is a permanent write load on SD-card-backed flash.
assert_contains '^[[:space:]]*target:[[:space:]]+/run/omt[[:space:]]*$' \
    "per-boot receiver state is mounted as tmpfs at /run/omt"
assert_contains '^[[:space:]]*size:[[:space:]]+[0-9]+[[:space:]]*$' \
    "the runtime tmpfs is size-capped so it cannot consume RAM"
# Left implicit, the mode is whatever the engine defaults to -- Docker 1777,
# Podman 700 owned by container root -- and the unprivileged image user cannot
# create its own directory under the latter, so the container never starts.
assert_contains '^[[:space:]]*mode:[[:space:]]+1023[[:space:]]*$' \
    "the runtime tmpfs mode is explicit rather than engine-dependent"
assert_not_contains '^[[:space:]]*-[[:space:]]+omt-config:/etc/omt/run' \
    "the runtime directory is not bound back onto the persistent volume"
assert_contains '^[[:space:]]*-[[:space:]]+"\$\{OMT_VIDEO_GID:-27\}"[[:space:]]*(#.*)?$' \
    "video group uses an installer-provided GID with the Alpine fallback"
assert_contains '^[[:space:]]*-[[:space:]]+"\$\{OMT_RENDER_GID:-27\}"[[:space:]]*(#.*)?$' \
    "render group uses an installer-provided GID with the Alpine fallback"
assert_contains '^[[:space:]]*-[[:space:]]+"\$\{OMT_AUDIO_GID:-18\}"[[:space:]]*(#.*)?$' \
    "audio group uses an installer-provided GID with the Alpine fallback"
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
