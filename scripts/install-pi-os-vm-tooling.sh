#!/bin/bash
# Install or provision the full-system Raspberry Pi OS VM prerequisites.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
PROJECT_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd -P)"
# shellcheck source=tests/vm/tooling.env
source "${PROJECT_ROOT}/tests/vm/tooling.env"

native_tools_ready() {
    local tool version major
    for tool in qemu-system-aarch64 guestfish virt-resize; do
        command -v "${tool}" >/dev/null 2>&1 || return 1
    done
    version="$(qemu-system-aarch64 --version | sed -n '1s/.*version \([0-9][0-9.]*\).*/\1/p')"
    major="${version%%.*}"
    [[ "${major}" =~ ^[0-9]+$ ]] && (( major >= 9 ))
}

if [[ "$(uname -s)" != "Linux" ]]; then
    echo "Full-system Raspberry Pi OS VM tooling is supported on Linux only."
    exit 0
fi
case "$(uname -m)" in
    x86_64|amd64) ;;
    *)
        echo "Full-system Raspberry Pi OS VM tooling is only provisioned on x86-64 hosts."
        exit 0
        ;;
esac

if native_tools_ready; then
    echo "Native Raspberry Pi OS VM tooling is installed."
    exit 0
fi

command -v podman >/dev/null 2>&1 || {
    echo "ERROR: Podman is required for the isolated Raspberry Pi OS VM tooling." >&2
    exit 1
}
podman info >/dev/null 2>&1 || {
    echo "ERROR: Podman is installed but unavailable for the current user." >&2
    exit 1
}

echo "Building isolated Raspberry Pi OS VM tooling (${PI_OS_VM_TOOLING_IMAGE})..."
podman build --pull=always --format docker \
    --build-arg "PI_OS_VM_TOOLING_BASE=${PI_OS_VM_TOOLING_BASE}" \
    --file "${PROJECT_ROOT}/tests/vm/Containerfile" \
    --tag "${PI_OS_VM_TOOLING_IMAGE}" \
    "${PROJECT_ROOT}/tests/vm"

podman run --rm --entrypoint /bin/bash "${PI_OS_VM_TOOLING_IMAGE}" -eu -c '
    command -v qemu-system-aarch64 >/dev/null
    command -v guestfish >/dev/null
    command -v virt-resize >/dev/null
    qemu-system-aarch64 --version
    qemu-system-aarch64 -machine help | grep -q "raspi3b"
    qemu-system-aarch64 -device usb-net,help >/dev/null
'
echo "Isolated Raspberry Pi OS VM tooling is installed and verified."
