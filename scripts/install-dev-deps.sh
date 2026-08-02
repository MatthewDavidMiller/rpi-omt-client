#!/bin/bash
# Install local development system dependencies.
# Usage: ./scripts/install-dev-deps.sh

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

OS=""
OS_LIKE=""
if [[ -f /etc/os-release ]]; then
    # shellcheck disable=SC1091
    . /etc/os-release
    OS="${ID:-}"
    OS_LIKE="${ID_LIKE:-}"
else
    OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
fi

install_apt_packages() {
    sudo apt-get update
    sudo apt-get install -y "$@"
}

install_dnf_packages() {
    sudo dnf install -y "$@"
}

install_pacman_packages() {
    sudo pacman -S --noconfirm "$@"
}

install_brew_packages() {
    brew install "$@"
}

echo "=== Installing Development Dependencies ==="
echo "Detected OS: ${OS}${OS_LIKE:+ (${OS_LIKE})}"

case "${OS}" in
    ubuntu|debian)
        install_apt_packages curl libguestfs-tools openssh-client podman \
            python3-venv qemu-system-arm shellcheck tar xz-utils
        ;;
    fedora)
        sudo dnf install -y --setopt=install_weak_deps=False \
            curl guestfs-tools kernel-core libguestfs openssh-clients podman \
            python3 qemu-system-aarch64-core ShellCheck tar xz
        ;;
    rocky|almalinux|rhel|centos)
        install_dnf_packages curl openssh-clients podman python3 ShellCheck tar xz
        ;;
    arch)
        install_pacman_packages curl podman python shellcheck tar
        ;;
    darwin)
        install_brew_packages coreutils curl python shellcheck
        ;;
    *)
        if [[ " ${OS_LIKE} " == *" debian "* ]]; then
            install_apt_packages curl libguestfs-tools openssh-client podman \
                python3-venv qemu-system-arm shellcheck tar xz-utils
        elif [[ " ${OS_LIKE} " == *" fedora "* ]] || \
             [[ " ${OS_LIKE} " == *" rhel "* ]] || \
             [[ " ${OS_LIKE} " == *" centos "* ]]; then
            install_dnf_packages curl openssh-clients podman python3 ShellCheck tar xz
        else
            echo -e "${YELLOW}WARN${NC}: Unknown OS. Install curl, SHA-512 tools, tar, Python venv, and ShellCheck manually."
        fi
        ;;
esac

if ! command -v hadolint >/dev/null 2>&1; then
    case "${OS}" in
        fedora|rocky|almalinux|rhel|centos)
            sudo dnf install -y hadolint || "${SCRIPT_DIR}/install-hadolint.sh"
            ;;
        arch)
            sudo pacman -S --noconfirm hadolint || "${SCRIPT_DIR}/install-hadolint.sh"
            ;;
        darwin)
            brew install hadolint
            ;;
        *)
            "${SCRIPT_DIR}/install-hadolint.sh"
            ;;
    esac
fi

if [[ "$(uname -s)" == "Linux" ]]; then
    "${SCRIPT_DIR}/install-arm64-emulation.sh"
    "${SCRIPT_DIR}/install-pi-os-vm-tooling.sh"
fi

echo ""
echo "=== Development Dependency Summary ==="
summary_tools=(curl python3 sha512sum shellcheck tar hadolint)
if [[ "$(uname -s)" == "Linux" ]]; then
    summary_tools+=(podman)
fi
for tool in "${summary_tools[@]}"; do
    if command -v "${tool}" >/dev/null 2>&1; then
        echo -e "  ${tool}: ${GREEN}installed${NC}"
    else
        echo -e "  ${tool}: ${RED}missing${NC}"
    fi
done

echo -e "  yamllint: ${GREEN}managed by tests/.venv${NC}"
echo -e "  .NET SDK: ${GREEN}managed in .build/dotnet${NC}"
if [[ "$(uname -s)" != "Linux" || ! "$(uname -m)" =~ ^(x86_64|amd64)$ ]]; then
    echo -e "  Pi OS VM: ${YELLOW}not provisioned on this host architecture${NC}"
elif command -v qemu-system-aarch64 >/dev/null 2>&1 && \
     command -v guestfish >/dev/null 2>&1 && command -v virt-resize >/dev/null 2>&1; then
    qemu_version="$(qemu-system-aarch64 --version | sed -n '1s/.*version \([0-9][0-9.]*\).*/\1/p')"
    qemu_major="${qemu_version%%.*}"
    if [[ "${qemu_major}" =~ ^[0-9]+$ ]] && (( qemu_major >= 9 )); then
        echo -e "  Pi OS VM: ${GREEN}native tooling installed${NC}"
    else
        echo -e "  Pi OS VM: ${GREEN}isolated Podman tooling installed${NC}"
    fi
else
    echo -e "  Pi OS VM: ${GREEN}isolated Podman tooling installed${NC}"
fi
