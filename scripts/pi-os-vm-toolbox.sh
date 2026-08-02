#!/bin/bash
# Execute the VM lifecycle inside the make-install-provisioned tooling image.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
PROJECT_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd -P)"
# shellcheck source=tests/vm/tooling.env
source "${PROJECT_ROOT}/tests/vm/tooling.env"

die() {
    echo "ERROR: $*" >&2
    exit 1
}

command -v podman >/dev/null 2>&1 || die "Podman is unavailable; run make install."
podman image exists "${PI_OS_VM_TOOLING_IMAGE}" || \
    die "Raspberry Pi OS VM tooling is not installed; run make install."

project_hash="$(printf '%s' "${PROJECT_ROOT}" | sha256sum | cut -c1-12)"
container_name="rpi-omt-pi-os-vm-tools-${project_hash}"

if ! podman container exists "${container_name}"; then
    podman run --detach \
        --name "${container_name}" \
        --network host \
        --userns keep-id \
        --user "$(id -u):$(id -g)" \
        --security-opt label=disable \
        --volume "${PROJECT_ROOT}:/workspace" \
        --workdir /workspace \
        "${PI_OS_VM_TOOLING_IMAGE}" >/dev/null
elif [[ "$(podman inspect --format '{{.State.Running}}' "${container_name}")" != "true" ]]; then
    podman start "${container_name}" >/dev/null
fi

container_state_dir=/workspace/.build/pi-os-vm
if [[ -n "${PI_OS_VM_STATE_DIR:-}" ]]; then
    case "${PI_OS_VM_STATE_DIR}" in
        "${PROJECT_ROOT}"/*)
            container_state_dir="/workspace/${PI_OS_VM_STATE_DIR#"${PROJECT_ROOT}"/}"
            ;;
        *)
            die "PI_OS_VM_STATE_DIR must stay inside ${PROJECT_ROOT} when using isolated tooling."
            ;;
    esac
fi

exec_args=(--interactive)
if [[ -t 0 && -t 1 ]]; then
    exec_args+=(--tty)
fi
podman exec "${exec_args[@]}" \
    --env PI_OS_VM_IN_TOOLBOX=1 \
    --env "PI_OS_VM_STATE_DIR=${container_state_dir}" \
    --env "PI_OS_VM_SSH_PORT_OVERRIDE=${PI_OS_VM_SSH_PORT_OVERRIDE:-}" \
    --env "PI_OS_VM_WEB_PORT_OVERRIDE=${PI_OS_VM_WEB_PORT_OVERRIDE:-}" \
    --env "PI_OS_VM_SSH_WAIT_SECONDS=${PI_OS_VM_SSH_WAIT_SECONDS:-}" \
    "${container_name}" ./scripts/pi-os-vm.sh "$@"
