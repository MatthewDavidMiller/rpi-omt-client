#!/bin/bash
# Fast contract tests for the heavyweight full-system Pi OS VM harness.

set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
RUNNER="${ROOT}/scripts/pi-os-vm.sh"
TOOLBOX="${ROOT}/scripts/pi-os-vm-toolbox.sh"
TOOLING_INSTALLER="${ROOT}/scripts/install-pi-os-vm-tooling.sh"
DEV_INSTALLER="${ROOT}/scripts/install-dev-deps.sh"
IMAGE_ENV="${ROOT}/tests/vm/pi-os-image.env"
TOOLING_ENV="${ROOT}/tests/vm/tooling.env"
CONTAINERFILE="${ROOT}/tests/vm/Containerfile"
FIRSTBOOT="${ROOT}/tests/vm/firstboot.sh"
GUEST_TEST="${ROOT}/tests/vm/run-in-guest.sh"

bash -n "${RUNNER}" "${TOOLBOX}" "${TOOLING_INSTALLER}" \
    "${FIRSTBOOT}" "${GUEST_TEST}"

if ! "${RUNNER}" --help | grep -q '^Usage:'; then
    echo "FAIL: Pi OS VM help is unavailable" >&2
    exit 1
fi
if "${RUNNER}" unknown-command >/dev/null 2>&1; then
    echo "FAIL: Pi OS VM runner accepted an unknown command" >&2
    exit 1
fi
if PI_OS_VM_SSH_PORT_OVERRIDE=22 "${RUNNER}" status >/dev/null 2>&1; then
    echo "FAIL: privileged SSH forward was accepted" >&2
    exit 1
fi

case_dir="$(mktemp -d)"
trap 'rm -rf "${case_dir}"' EXIT
if PI_OS_VM_IN_TOOLBOX=1 PI_OS_VM_STATE_DIR="${case_dir}/pi-os-vm" \
        "${RUNNER}" status \
        > "${case_dir}/status" 2>&1; then
    echo "FAIL: an absent VM was reported as running" >&2
    exit 1
fi
grep -Fxq 'VM process: stopped' "${case_dir}/status"

# shellcheck source=tests/vm/pi-os-image.env
source "${IMAGE_ENV}"
# shellcheck source=tests/vm/tooling.env
source "${TOOLING_ENV}"
[[ "${PI_OS_IMAGE_URL}" == https://downloads.raspberrypi.com/* ]] || {
    echo "FAIL: VM image is not fetched from Raspberry Pi over HTTPS" >&2
    exit 1
}
[[ "${PI_OS_IMAGE_SHA256}" =~ ^[0-9a-f]{64}$ ]] || {
    echo "FAIL: VM image SHA-256 is not pinned" >&2
    exit 1
}
[[ "${PI_OS_QEMU_MACHINE}" == "raspi3b" ]] || {
    echo "FAIL: tested QEMU board contract changed" >&2
    exit 1
}
[[ "${PI_OS_IMAGE_NAME}" == *-full.img && "${PI_OS_IMAGE_URL}" == */raspios_full_arm64/* ]] || {
    echo "FAIL: VM is not pinned to the full Raspberry Pi OS 64-bit image" >&2
    exit 1
}
(( PI_OS_VM_DISK_BYTES >= 17179869184 )) || {
    echo "FAIL: full Raspberry Pi OS VM disk is smaller than 16 GiB" >&2
    exit 1
}
[[ "${PI_OS_VM_TOOLING_BASE}" =~ @sha256:[0-9a-f]{64}$ ]] || {
    echo "FAIL: Fedora VM-tooling base image is not digest pinned" >&2
    exit 1
}

grep -Fq "printf '%s  %s\\n' \"\${PI_OS_IMAGE_SHA256}\"" "${RUNNER}" || {
    echo "FAIL: download is not checked against the pinned SHA-256" >&2
    exit 1
}
grep -Fq 'hostfwd=tcp:127.0.0.1:' "${RUNNER}" || {
    echo "FAIL: VM forwards are not restricted to loopback" >&2
    exit 1
}
grep -Fq 'deploy/manifest-v2.txt' "${RUNNER}" || {
    echo "FAIL: VM upload does not use the deployment manifest boundary" >&2
    exit 1
}
grep -Fq 'image_sha256=${PI_OS_IMAGE_SHA256}' "${RUNNER}" || {
    echo "FAIL: persistent VM disks are not correlated to the pinned full image" >&2
    exit 1
}
grep -Fq 'pi-os-vm-toolbox.sh' "${RUNNER}" || {
    echo "FAIL: VM lifecycle does not delegate when native tooling is unavailable" >&2
    exit 1
}
grep -Fq 'install-pi-os-vm-tooling.sh' "${DEV_INSTALLER}" || {
    echo "FAIL: make install does not provision full-system VM tooling" >&2
    exit 1
}
for package in qemu-system-aarch64-core libguestfs guestfs-tools kernel-core; do
    grep -Fq "${package}" "${CONTAINERFILE}" || {
        echo "FAIL: isolated VM tooling omits ${package}" >&2
        exit 1
    }
done
grep -Fq -- '--network host' "${TOOLBOX}" || {
    echo "FAIL: isolated QEMU networking cannot reach host loopback forwards" >&2
    exit 1
}
grep -Fq -- '--userns keep-id' "${TOOLBOX}" || {
    echo "FAIL: isolated tooling does not preserve host checkout ownership" >&2
    exit 1
}
grep -Fq 'USER 1000:1000' "${CONTAINERFILE}" || {
    echo "FAIL: isolated VM tooling runs as root" >&2
    exit 1
}
grep -Fq 'sha256sum | cut -c1-12' "${TOOLBOX}" || {
    echo "FAIL: VM tooling container is not scoped to one project checkout" >&2
    exit 1
}
if grep -Fq 'vars.yml' "${RUNNER}"; then
    echo "FAIL: sensitive vars.yml must never enter the VM capsule" >&2
    exit 1
fi
grep -Fq 'PasswordAuthentication no' "${FIRSTBOOT}" || {
    echo "FAIL: VM SSH password authentication is not disabled" >&2
    exit 1
}
if ! grep -Fq '/etc/rpi-issue' "${GUEST_TEST}" || \
   ! grep -Fq 'raspberrypi-sys-mods' "${GUEST_TEST}"; then
    echo "FAIL: guest suite does not prove it booted Raspberry Pi OS" >&2
    exit 1
fi
grep -Fq 'systemd-analyze verify' "${GUEST_TEST}" || {
    echo "FAIL: guest suite does not verify installed systemd units" >&2
    exit 1
}
grep -Fq 'multi-user.target' "${GUEST_TEST}" || {
    echo "FAIL: guest suite does not test full-image headless conversion" >&2
    exit 1
}
grep -Fq 'request-triggered host diagnostics' "${GUEST_TEST}" || {
    echo "FAIL: guest suite does not exercise diagnostics path activation" >&2
    exit 1
}
grep -Fq 'invalid action without rebooting' "${GUEST_TEST}" || {
    echo "FAIL: guest suite does not safely exercise the reboot path" >&2
    exit 1
}

echo "Pi OS VM harness contract tests passed"
