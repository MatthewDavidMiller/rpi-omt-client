#!/bin/bash
# Full-system Raspberry Pi OS VM lifecycle and integration test runner.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
PROJECT_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd -P)"
VM_FILES_DIR="${PROJECT_ROOT}/tests/vm"
# shellcheck source=tests/vm/pi-os-image.env
source "${VM_FILES_DIR}/pi-os-image.env"

# The direct backend needs no host libvirt daemon and also works inside the
# documented EL10/Fedora Podman tooling environment.
export LIBGUESTFS_BACKEND="${LIBGUESTFS_BACKEND:-direct}"

STATE_DIR="${PI_OS_VM_STATE_DIR:-${PROJECT_ROOT}/.build/pi-os-vm}"
ARCHIVE="${STATE_DIR}/${PI_OS_IMAGE_NAME}.xz"
BASE_DISK="${STATE_DIR}/${PI_OS_IMAGE_NAME}"
VM_DISK="${STATE_DIR}/disk.img"
VM_METADATA="${STATE_DIR}/disk-image.env"
VM_KERNEL="${STATE_DIR}/kernel8.img"
VM_DTB="${STATE_DIR}/bcm2710-rpi-3-b.dtb"
VM_KEY="${STATE_DIR}/id_ed25519"
VM_PID_FILE="${STATE_DIR}/qemu.pid"
VM_SERIAL_LOG="${STATE_DIR}/serial.log"
VM_CAPSULE="${STATE_DIR}/deployment-capsule.tar"
SSH_PORT="${PI_OS_VM_SSH_PORT_OVERRIDE:-${PI_OS_VM_SSH_PORT}}"
WEB_PORT="${PI_OS_VM_WEB_PORT_OVERRIDE:-${PI_OS_VM_WEB_PORT}}"
SSH_WAIT_SECONDS="${PI_OS_VM_SSH_WAIT_SECONDS:-600}"

usage() {
    cat <<'EOF'
Usage: scripts/pi-os-vm.sh COMMAND

Commands:
  prepare   Download, verify, resize, and provision the Raspberry Pi OS image
  start     Start the VM and wait for public-key SSH
  stop      Shut down only this VM, with a process-identity-checked fallback
  status    Report whether the VM process and SSH endpoint are available
  shell     Open an interactive SSH session in the VM
  test      Upload the deployment capsule and run the in-guest integration suite
  debug     Collect a bounded host/guest diagnostic report under .build/pi-os-vm
  help      Show this help

The VM is persistent. Run prepare once, then start/test/stop as needed. Set
PI_OS_VM_STATE_DIR, PI_OS_VM_SSH_PORT_OVERRIDE, or PI_OS_VM_WEB_PORT_OVERRIDE
to isolate concurrent instances. The ARM64 deployment tarball must already
exist before `test`; build it with `make build-arm64`.
On hosts without native cross-QEMU, make install provisions isolated Podman
tooling and these commands delegate to it automatically.
EOF
}

die() {
    echo "ERROR: $*" >&2
    exit 1
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || die "Required command is unavailable: $1"
}

validate_integer() {
    local name="$1" value="$2" minimum="$3" maximum="$4"
    [[ "${value}" =~ ^[0-9]+$ ]] || die "${name} must be an integer."
    (( 10#${value} >= minimum && 10#${value} <= maximum )) || \
        die "${name} must be between ${minimum} and ${maximum}."
}

validate_configuration() {
    [[ "${PI_OS_IMAGE_URL}" == https://downloads.raspberrypi.com/* ]] || \
        die "Pi OS image URL must use downloads.raspberrypi.com over HTTPS."
    [[ "${PI_OS_IMAGE_SHA256}" =~ ^[0-9a-f]{64}$ ]] || die "Invalid Pi OS image SHA-256."
    [[ "${PI_OS_IMAGE_NAME}" =~ ^[A-Za-z0-9._-]+\.img$ ]] || die "Invalid Pi OS image name."
    [[ "${PI_OS_QEMU_MACHINE}" == "raspi3b" ]] || die "Unsupported QEMU machine."
    validate_integer PI_OS_VM_DISK_BYTES "${PI_OS_VM_DISK_BYTES}" 4294967296 34359738368
    validate_integer PI_OS_VM_SSH_PORT "${SSH_PORT}" 1024 65535
    validate_integer PI_OS_VM_WEB_PORT "${WEB_PORT}" 1024 65535
    validate_integer PI_OS_VM_SSH_WAIT_SECONDS "${SSH_WAIT_SECONDS}" 30 3600
    [[ "${SSH_PORT}" != "${WEB_PORT}" ]] || die "SSH and Web forwarded ports must differ."
    [[ -n "${STATE_DIR}" && "${STATE_DIR}" != "/" && "${STATE_DIR}" != "${PROJECT_ROOT}" ]] || \
        die "Refusing unsafe VM state directory: ${STATE_DIR:-<empty>}"
}

native_vm_tools_available() {
    local tool version major
    for tool in qemu-system-aarch64 guestfish virt-resize; do
        command -v "${tool}" >/dev/null 2>&1 || return 1
    done
    version="$(qemu-system-aarch64 --version | sed -n '1s/.*version \([0-9][0-9.]*\).*/\1/p')"
    major="${version%%.*}"
    [[ "${major}" =~ ^[0-9]+$ ]] && (( major >= PI_OS_QEMU_MIN_MAJOR ))
}

maybe_delegate_to_toolbox() {
    case "${1:-help}" in
        help|-h|--help) return 0 ;;
    esac
    [[ "${PI_OS_VM_IN_TOOLBOX:-0}" != "1" ]] || return 0
    native_vm_tools_available && return 0
    exec "${SCRIPT_DIR}/pi-os-vm-toolbox.sh" "$@"
}

read_vm_pid() {
    local pid=""
    if [[ -f "${VM_PID_FILE}" && ! -L "${VM_PID_FILE}" ]]; then
        IFS= read -r pid < "${VM_PID_FILE}" || true
    fi
    [[ "${pid}" =~ ^[1-9][0-9]*$ ]] || return 1
    printf '%s\n' "${pid}"
}

vm_process_is_running() {
    local pid executable
    pid="$(read_vm_pid)" || return 1
    kill -0 "${pid}" 2>/dev/null || return 1
    executable="$(readlink -f "/proc/${pid}/exe" 2>/dev/null || true)"
    [[ "$(basename -- "${executable}")" == "qemu-system-aarch64" ]]
}

ssh_options=(
    -i "${VM_KEY}"
    -p "${SSH_PORT}"
    -o BatchMode=yes
    -o ConnectTimeout=5
    -o StrictHostKeyChecking=no
    -o UserKnownHostsFile=/dev/null
    -o LogLevel=ERROR
)

vm_ssh() {
    ssh "${ssh_options[@]}" omtvm@127.0.0.1 "$@"
}

vm_scp() {
    scp -i "${VM_KEY}" -P "${SSH_PORT}" \
        -o BatchMode=yes -o ConnectTimeout=10 -o StrictHostKeyChecking=no \
        -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR "$@"
}

wait_for_ssh() {
    local waited=0
    while (( waited < SSH_WAIT_SECONDS )); do
        vm_process_is_running || die "QEMU exited before SSH became ready; inspect ${VM_SERIAL_LOG}."
        if vm_ssh test -f /var/lib/omt-vm/firstboot-complete >/dev/null 2>&1; then
            echo "Raspberry Pi OS VM is ready on SSH port ${SSH_PORT}."
            return 0
        fi
        sleep 2
        ((waited += 2))
        if (( waited % 30 == 0 )); then
            echo "Waiting for Raspberry Pi OS boot and first-boot provisioning (${waited}s)..."
        fi
    done
    die "SSH did not become ready within ${SSH_WAIT_SECONDS}s; inspect ${VM_SERIAL_LOG}."
}

check_qemu_version() {
    local version major
    version="$(qemu-system-aarch64 --version | sed -n '1s/.*version \([0-9][0-9.]*\).*/\1/p')"
    major="${version%%.*}"
    [[ "${major}" =~ ^[0-9]+$ ]] || die "Could not parse the QEMU version."
    (( major >= PI_OS_QEMU_MIN_MAJOR )) || \
        die "QEMU ${PI_OS_QEMU_MIN_MAJOR} or newer is required; found ${version}."
}

validate_disk_metadata() {
    [[ -f "${VM_METADATA}" && ! -L "${VM_METADATA}" ]] || \
        die "VM disk has no trusted image metadata. Move ${VM_DISK} aside and prepare the pinned full image."
    grep -Fxq "image_name=${PI_OS_IMAGE_NAME}" "${VM_METADATA}" && \
        grep -Fxq "image_sha256=${PI_OS_IMAGE_SHA256}" "${VM_METADATA}" && \
        grep -Fxq "disk_bytes=${PI_OS_VM_DISK_BYTES}" "${VM_METADATA}" || \
        die "VM disk does not match the pinned full Raspberry Pi OS image. Move it aside and prepare again."
}

cmd_prepare() {
    require_command curl
    require_command guestfish
    require_command qemu-system-aarch64
    require_command sha256sum
    require_command ssh-keygen
    require_command virt-resize
    require_command xz
    check_qemu_version
    mkdir -p "${STATE_DIR}"

    if [[ ! -f "${ARCHIVE}" ]]; then
        echo "Downloading pinned Raspberry Pi OS Full image..."
        curl --fail --location --proto '=https' --tlsv1.2 \
            --retry 3 --continue-at - --output "${ARCHIVE}.part" "${PI_OS_IMAGE_URL}"
        mv -f -- "${ARCHIVE}.part" "${ARCHIVE}"
    fi
    printf '%s  %s\n' "${PI_OS_IMAGE_SHA256}" "${ARCHIVE}" | sha256sum --check --status || \
        die "Pi OS archive checksum failed. Remove ${ARCHIVE} and retry."

    if [[ -f "${VM_DISK}" && ! -f "${VM_KEY}" ]]; then
        die "VM disk exists but its private SSH key is missing. Move ${VM_DISK} aside and prepare a new disk."
    fi
    if [[ -f "${VM_DISK}" ]]; then
        validate_disk_metadata
    fi
    if [[ ! -f "${VM_KEY}" ]]; then
        ssh-keygen -q -t ed25519 -N '' -C rpi-omt-pi-os-vm -f "${VM_KEY}"
    elif [[ ! -f "${VM_KEY}.pub" ]]; then
        ssh-keygen -y -f "${VM_KEY}" > "${VM_KEY}.pub"
    fi

    if [[ ! -f "${VM_DISK}" ]]; then
        if [[ ! -f "${BASE_DISK}" ]]; then
            echo "Decompressing verified Raspberry Pi OS image..."
            xz --decompress --stdout "${ARCHIVE}" > "${BASE_DISK}.tmp"
            mv -f -- "${BASE_DISK}.tmp" "${BASE_DISK}"
        fi
        echo "Expanding the VM root filesystem to $((PI_OS_VM_DISK_BYTES / 1024 / 1024 / 1024)) GiB..."
        truncate -s "${PI_OS_VM_DISK_BYTES}" "${VM_DISK}.tmp"
        virt-resize --quiet --expand /dev/sda2 "${BASE_DISK}" "${VM_DISK}.tmp"

        echo "Injecting one-time, key-only SSH provisioning..."
        guestfish --rw -a "${VM_DISK}.tmp" -i <<EOF
upload "${VM_FILES_DIR}/firstboot.sh" /usr/local/sbin/omt-vm-firstboot
chmod 0755 /usr/local/sbin/omt-vm-firstboot
upload "${VM_FILES_DIR}/omt-vm-firstboot.service" /etc/systemd/system/omt-vm-firstboot.service
chmod 0644 /etc/systemd/system/omt-vm-firstboot.service
mkdir-p /etc/systemd/system/multi-user.target.wants
ln-s /etc/systemd/system/omt-vm-firstboot.service /etc/systemd/system/multi-user.target.wants/omt-vm-firstboot.service
upload "${VM_KEY}.pub" /etc/omt-vm-authorized-key
chmod 0600 /etc/omt-vm-authorized-key
upload "${VM_FILES_DIR}/omt-vm-modules.conf" /etc/modules-load.d/omt-vm.conf
chmod 0644 /etc/modules-load.d/omt-vm.conf
EOF
        mv -f -- "${VM_DISK}.tmp" "${VM_DISK}"
        {
            printf 'image_name=%s\n' "${PI_OS_IMAGE_NAME}"
            printf 'image_sha256=%s\n' "${PI_OS_IMAGE_SHA256}"
            printf 'disk_bytes=%s\n' "${PI_OS_VM_DISK_BYTES}"
        } > "${VM_METADATA}.tmp"
        mv -f -- "${VM_METADATA}.tmp" "${VM_METADATA}"
        # The verified compressed archive is the durable cache. The expanded
        # source image is recoverable and would otherwise consume another 9 GiB.
        rm -f -- "${BASE_DISK}"
    fi

    echo "Prepared persistent VM disk: ${VM_DISK}"
    echo "Start it with: make pi-os-vm-start"
}

extract_boot_files() {
    guestfish --ro -a "${VM_DISK}" -i <<EOF
download /boot/firmware/kernel8.img ${VM_KERNEL}.tmp
download /boot/firmware/bcm2710-rpi-3-b.dtb ${VM_DTB}.tmp
EOF
    mv -f -- "${VM_KERNEL}.tmp" "${VM_KERNEL}"
    mv -f -- "${VM_DTB}.tmp" "${VM_DTB}"
}

cmd_start() {
    require_command guestfish
    require_command qemu-system-aarch64
    require_command ssh
    check_qemu_version
    [[ -f "${VM_DISK}" && -f "${VM_KEY}" ]] || \
        die "VM is not prepared. Run: make pi-os-vm-prepare"
    validate_disk_metadata
    if vm_process_is_running; then
        echo "Raspberry Pi OS VM is already running."
        wait_for_ssh
        return 0
    fi
    mkdir -p "${STATE_DIR}"
    rm -f -- "${VM_PID_FILE}"
    extract_boot_files
    : > "${VM_SERIAL_LOG}"
    qemu-system-aarch64 \
        -machine "${PI_OS_QEMU_MACHINE}" \
        -cpu cortex-a53 \
        -smp 4 \
        -accel tcg,thread=multi \
        -kernel "${VM_KERNEL}" \
        -dtb "${VM_DTB}" \
        -append 'root=/dev/mmcblk0p2 rootwait rw fsck.repair=yes net.ifnames=0 console=ttyAMA0,115200 console=tty1 systemd.show_status=true' \
        -drive "file=${VM_DISK},format=raw,if=sd,cache=writeback" \
        -netdev "user,id=vmnet,hostfwd=tcp:127.0.0.1:${SSH_PORT}-:22,hostfwd=tcp:127.0.0.1:${WEB_PORT}-:5000" \
        -usb \
        -device usb-net,netdev=vmnet \
        -display none \
        -monitor none \
        -serial "file:${VM_SERIAL_LOG}" \
        -daemonize \
        -pidfile "${VM_PID_FILE}"
    wait_for_ssh
    echo "Forwarded Web UI (when the service has devices): https://127.0.0.1:${WEB_PORT}"
}

cmd_stop() {
    local pid waited=0
    if ! vm_process_is_running; then
        echo "Raspberry Pi OS VM is not running."
        return 0
    fi
    pid="$(read_vm_pid)"
    vm_ssh sudo systemctl poweroff --no-block >/dev/null 2>&1 || true
    while kill -0 "${pid}" 2>/dev/null && (( waited < 60 )); do
        sleep 1
        ((waited += 1))
    done
    if kill -0 "${pid}" 2>/dev/null; then
        # vm_process_is_running re-checks /proc/PID/exe before signaling, so a
        # stale pidfile can never select an unrelated host process.
        vm_process_is_running || die "VM pidfile no longer identifies QEMU; refusing to signal PID ${pid}."
        kill -TERM "${pid}"
        for _ in {1..20}; do
            kill -0 "${pid}" 2>/dev/null || break
            sleep 1
        done
    fi
    kill -0 "${pid}" 2>/dev/null && die "QEMU did not stop; inspect ${VM_SERIAL_LOG}."
    rm -f -- "${VM_PID_FILE}"
    echo "Raspberry Pi OS VM stopped."
}

cmd_status() {
    if ! vm_process_is_running; then
        echo "VM process: stopped"
        return 1
    fi
    echo "VM process: running (PID $(read_vm_pid))"
    if vm_ssh true >/dev/null 2>&1; then
        echo "SSH: ready on 127.0.0.1:${SSH_PORT}"
        echo "Web forward: https://127.0.0.1:${WEB_PORT}"
    else
        echo "SSH: booting or unavailable"
    fi
}

build_capsule() {
    local manifest="${PROJECT_ROOT}/deploy/manifest-v2.txt" entry
    local -a files=()
    [[ -f "${PROJECT_ROOT}/omt-client-arm64.tar.gz" ]] || \
        die "omt-client-arm64.tar.gz is missing. Run: make build-arm64"
    while IFS= read -r entry; do
        [[ "${entry}" == "version=2" ]] && continue
        [[ "${entry}" =~ ^[A-Za-z0-9._/-]+$ && "${entry}" != /* && "${entry}" != *../* ]] || \
            die "Unsafe deployment manifest entry: ${entry}"
        [[ -f "${PROJECT_ROOT}/${entry}" ]] || die "Manifest file is missing: ${entry}"
        files+=("${entry}")
    done < "${manifest}"
    (cd "${PROJECT_ROOT}" && tar -cf "${VM_CAPSULE}.tmp" "${files[@]}")
    mv -f -- "${VM_CAPSULE}.tmp" "${VM_CAPSULE}"
}

cmd_debug() {
    local timestamp report
    mkdir -p "${STATE_DIR}"
    timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
    report="${STATE_DIR}/debug-${timestamp}.txt"
    umask 077
    {
        echo "Raspberry Pi OS VM debug report"
        echo "captured_at_utc=${timestamp}"
        echo "machine=${PI_OS_QEMU_MACHINE}"
        echo "ssh_port=${SSH_PORT}"
        echo "qemu_version=$(qemu-system-aarch64 --version 2>/dev/null | head -n 1 || true)"
        echo
        echo "## serial tail"
        tail -n 400 "${VM_SERIAL_LOG}" 2>/dev/null || echo unavailable
        if vm_process_is_running && vm_ssh true >/dev/null 2>&1; then
            echo
            echo "## guest state"
            vm_ssh 'set -o pipefail; uname -a; cat /etc/os-release; systemctl --failed --no-pager; systemctl status omt-client.service omt-client-host-diagnostics.path omt-client-reboot.path --no-pager -l 2>&1 | tail -n 300; find /dev/dri /dev/snd -maxdepth 1 -type c -exec stat -c "device=%n uid=%u gid=%g mode=%a" {} + 2>&1; docker version 2>&1 | sed -n "1,120p"; journalctl -b -u omt-vm-firstboot.service -u omt-client.service -u omt-client-host-diagnostics.service -u omt-client-reboot.service --no-pager -n 400 -o short-iso 2>&1'
        else
            echo "guest_unavailable=yes"
        fi
    } > "${report}"
    chmod 0600 "${report}"
    echo "Collected VM diagnostics: ${report}"
}

cmd_test() {
    local guest_capsule=/home/omtvm/deployment-capsule.tar
    local guest_root=/home/omtvm/rpi-omt-client
    vm_process_is_running || cmd_start
    wait_for_ssh
    build_capsule
    echo "Uploading manifest-bounded deployment capsule..."
    vm_scp "${VM_CAPSULE}" "omtvm@127.0.0.1:${guest_capsule}"
    vm_scp "${VM_FILES_DIR}/run-in-guest.sh" "omtvm@127.0.0.1:/home/omtvm/run-in-guest.sh"
    vm_ssh mkdir -p "${guest_root}"
    vm_ssh tar -xf "${guest_capsule}" -C "${guest_root}"
    if ! vm_ssh bash /home/omtvm/run-in-guest.sh "${guest_root}"; then
        echo "VM integration failed; collecting diagnostics..." >&2
        cmd_debug >&2 || true
        return 1
    fi
}

main() {
    validate_configuration
    maybe_delegate_to_toolbox "$@"
    case "${1:-help}" in
        prepare) [[ $# -eq 1 ]] || die "prepare accepts no arguments"; cmd_prepare ;;
        start) [[ $# -eq 1 ]] || die "start accepts no arguments"; cmd_start ;;
        stop) [[ $# -eq 1 ]] || die "stop accepts no arguments"; cmd_stop ;;
        status) [[ $# -eq 1 ]] || die "status accepts no arguments"; cmd_status ;;
        shell)
            [[ $# -eq 1 ]] || die "shell accepts no arguments"
            vm_process_is_running || die "VM is not running."
            exec ssh "${ssh_options[@]}" omtvm@127.0.0.1
            ;;
        test) [[ $# -eq 1 ]] || die "test accepts no arguments"; cmd_test ;;
        debug) [[ $# -eq 1 ]] || die "debug accepts no arguments"; cmd_debug ;;
        help|-h|--help) usage ;;
        *) usage >&2; die "Unknown command: $1" ;;
    esac
}

main "$@"
