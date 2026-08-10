#!/bin/bash
# Install persistent ARM64 user-mode emulation for Linux x86-64 development hosts.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BINFMT_IMAGE="docker.io/tonistiigi/binfmt@sha256:400a4873b838d1b89194d982c45e5fb3cda4593fbfd7e08a02e76b03b21166f0"
EMULATOR_SHA256="1ad17b7bd5e15ce60075d0994d5c5e3914d16899a1e3119040b5c9e76e067f24"
EMULATOR_PATH="/usr/local/bin/qemu-aarch64-static"
BINFMT_CONFIG_SOURCE="${SCRIPT_DIR}/arm64-binfmt.conf"
BINFMT_CONFIG_PATH="/etc/binfmt.d/30-rpi-omt-qemu-aarch64.conf"
CHECK_IMAGE="docker.io/library/debian:bookworm-slim@sha256:4724b8cc51e33e398f0e2e15e18d5ec2851ff0c2280647e1310bc1642182655d"
EXTRACT_PATH="/usr/bin/qemu-aarch64"
CONTAINER_ID=""

usage() {
    echo "Usage: $0"
    echo "Installs persistent ARM64 emulation on a systemd-based Linux x86-64 host."
}

if [[ $# -gt 0 ]]; then
    case "$1" in
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            exit 2
            ;;
    esac
fi

if [[ "$(uname -s)" != "Linux" ]]; then
    echo "ERROR: persistent ARM64 emulation setup is supported on Linux only." >&2
    exit 1
fi

case "$(uname -m)" in
    aarch64|arm64)
        echo "ARM64 emulation is unnecessary on this native ARM64 host."
        exit 0
        ;;
    x86_64|amd64) ;;
    *)
        echo "ERROR: ARM64 emulation setup supports Linux x86-64 hosts only." >&2
        exit 1
        ;;
esac

for tool in install mktemp sha256sum systemctl timeout; do
    command -v "${tool}" >/dev/null 2>&1 || {
        echo "ERROR: ${tool} is required to install ARM64 emulation." >&2
        exit 1
    }
done
[[ -f "${BINFMT_CONFIG_SOURCE}" ]] || {
    echo "ERROR: missing binfmt configuration: ${BINFMT_CONFIG_SOURCE}" >&2
    exit 1
}
[[ -d /run/systemd/system ]] || {
    echo "ERROR: systemd must be running to persist ARM64 emulation across reboots." >&2
    exit 1
}

if [[ "${EUID}" -eq 0 ]]; then
    SUDO=()
else
    command -v sudo >/dev/null 2>&1 || {
        echo "ERROR: sudo is required to install host ARM64 emulation." >&2
        exit 1
    }
    sudo -v
    SUDO=(sudo -n)
fi

CONTAINER_ENGINE=""
for candidate in podman docker; do
    if command -v "${candidate}" >/dev/null 2>&1 &&
       "${candidate}" info >/dev/null 2>&1 &&
       "${SUDO[@]}" "${candidate}" info >/dev/null 2>&1; then
        CONTAINER_ENGINE="${candidate}"
        break
    fi
done
[[ -n "${CONTAINER_ENGINE}" ]] || {
    echo "ERROR: a working Podman or Docker installation is required." >&2
    exit 1
}

TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/rpi-omt-arm64-emulation.XXXXXX")"
cleanup() {
    if [[ -n "${CONTAINER_ID}" ]]; then
        "${SUDO[@]}" "${CONTAINER_ENGINE}" rm -f "${CONTAINER_ID}" >/dev/null 2>&1 || true
    fi
    rm -rf -- "${TEMP_DIR}"
}
trap cleanup EXIT

echo "Installing the pinned ARM64 emulator with ${CONTAINER_ENGINE}..."
"${SUDO[@]}" "${CONTAINER_ENGINE}" pull "${BINFMT_IMAGE}" >/dev/null
CONTAINER_ID="$("${SUDO[@]}" "${CONTAINER_ENGINE}" create "${BINFMT_IMAGE}")"
"${SUDO[@]}" "${CONTAINER_ENGINE}" cp \
    "${CONTAINER_ID}:${EXTRACT_PATH}" "${TEMP_DIR}/qemu-aarch64-static"
"${SUDO[@]}" "${CONTAINER_ENGINE}" rm "${CONTAINER_ID}" >/dev/null
CONTAINER_ID=""

printf '%s  %s\n' "${EMULATOR_SHA256}" "${TEMP_DIR}/qemu-aarch64-static" |
    sha256sum -c -

"${SUDO[@]}" install -d -m 0755 /usr/local/bin /etc/binfmt.d
"${SUDO[@]}" install -m 0755 \
    "${TEMP_DIR}/qemu-aarch64-static" "${EMULATOR_PATH}"
if command -v restorecon >/dev/null 2>&1; then
    "${SUDO[@]}" restorecon -F "${EMULATOR_PATH}"
fi
"${SUDO[@]}" install -m 0644 "${BINFMT_CONFIG_SOURCE}" "${BINFMT_CONFIG_PATH}"
"${SUDO[@]}" systemctl restart systemd-binfmt.service

handler=/proc/sys/fs/binfmt_misc/qemu-aarch64
[[ -r "${handler}" ]] || {
    echo "ERROR: systemd-binfmt did not register the qemu-aarch64 handler." >&2
    exit 1
}
grep -Fxq "interpreter ${EMULATOR_PATH}" "${handler}" || {
    echo "ERROR: qemu-aarch64 is registered with an unexpected interpreter." >&2
    exit 1
}
grep -Eq '^flags: .*F' "${handler}" || {
    echo "ERROR: qemu-aarch64 registration must use the fix-binary flag." >&2
    exit 1
}

# The machine name is the container's command, not an entrypoint override, so
# this is the same probe scripts/check-arm64-emulation.sh runs.
if [[ "$(timeout 120 "${CONTAINER_ENGINE}" run --rm --platform linux/arm64 \
    "${CHECK_IMAGE}" uname -m 2>/dev/null | tail -n 1)" != "aarch64" ]]; then
    echo "ERROR: ${CONTAINER_ENGINE} could not execute the ARM64 verification image." >&2
    exit 1
fi

echo "Persistent ARM64 emulation is installed and verified for ${CONTAINER_ENGINE}."
