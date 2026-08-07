#!/bin/bash
# Install local development system dependencies.
# Usage: ./scripts/install-dev-deps.sh

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

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

# The deployment application ships for Windows as well, and it is cross-built
# here so one Linux workstation can publish both packages and gate both on
# every commit.
install_windows_cross_toolchain() {
    case "${OS}" in
        ubuntu|debian)
            install_apt_packages mingw-w64
            ;;
        fedora)
            sudo dnf install -y --setopt=install_weak_deps=False \
                mingw64-gcc mingw64-winpthreads-static
            ;;
        rocky|almalinux|rhel|centos)
            # The mingw packages live in CRB, which is not enabled everywhere.
            sudo dnf install -y --enablerepo=crb \
                mingw64-gcc mingw64-winpthreads-static
            ;;
        arch)
            install_pacman_packages mingw-w64-gcc
            ;;
        darwin)
            install_brew_packages mingw-w64
            ;;
        *)
            if [[ " ${OS_LIKE} " == *" debian "* ]]; then
                install_apt_packages mingw-w64
            elif [[ " ${OS_LIKE} " == *" fedora "* ]] || \
                 [[ " ${OS_LIKE} " == *" rhel "* ]] || \
                 [[ " ${OS_LIKE} " == *" centos "* ]]; then
                sudo dnf install -y --enablerepo=crb \
                    mingw64-gcc mingw64-winpthreads-static
            else
                echo -e "${YELLOW}WARN${NC}: Unknown OS. Install the x86_64-w64-mingw32 toolchain manually."
            fi
            ;;
    esac
}

echo "=== Installing Development Dependencies ==="
echo "Detected OS: ${OS}${OS_LIKE:+ (${OS_LIKE})}"

case "${OS}" in
    ubuntu|debian)
        install_apt_packages alsa-utils curl gcc libasound2-dev \
            libssl-dev libx11-dev openssh-client pkg-config \
            podman python3-venv shellcheck tar xz-utils
        ;;
    fedora)
        sudo dnf install -y --setopt=install_weak_deps=False \
            alsa-lib-devel curl gcc libX11-devel openssl-devel \
            openssh-clients pkgconf-pkg-config podman python3 \
            ShellCheck tar xz
        ;;
    rocky|almalinux|rhel|centos)
        install_dnf_packages alsa-lib-devel curl gcc \
            libX11-devel openssh-clients openssl-devel pkgconf-pkg-config podman \
            python3 ShellCheck tar xz
        ;;
    arch)
        install_pacman_packages alsa-lib curl gcc libx11 \
            openssh openssl pkgconf podman python shellcheck tar
        ;;
    darwin)
        install_brew_packages coreutils curl openssl pkg-config python shellcheck
        ;;
    *)
        if [[ " ${OS_LIKE} " == *" debian "* ]]; then
            install_apt_packages alsa-utils curl gcc libasound2-dev \
                libssl-dev libx11-dev openssh-client pkg-config \
                podman python3-venv shellcheck tar xz-utils
        elif [[ " ${OS_LIKE} " == *" fedora "* ]] || \
             [[ " ${OS_LIKE} " == *" rhel "* ]] || \
             [[ " ${OS_LIKE} " == *" centos "* ]]; then
            install_dnf_packages alsa-lib-devel curl gcc \
                libX11-devel openssh-clients openssl-devel pkgconf-pkg-config \
                podman python3 ShellCheck tar xz
        else
            echo -e "${YELLOW}WARN${NC}: Unknown OS. Install Rust, OpenSSL/X11 headers, Python venv, and ShellCheck manually."
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

if ! command -v rustup >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
    # shellcheck disable=SC1091
    . "${HOME}/.cargo/env"
fi
# rust-toolchain.toml is the single source of truth for the pin, so the
# installer cannot drift from what the gates actually resolve.
RUST_PIN="$(
    sed -n 's/^[[:space:]]*channel[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' \
        "${PROJECT_ROOT}/rust-toolchain.toml"
)"
if [[ -z "${RUST_PIN}" ]]; then
    echo -e "${RED}ERROR${NC}: rust-toolchain.toml does not pin a channel" >&2
    exit 1
fi
rustup toolchain install "${RUST_PIN}" --profile minimal --component clippy,rustfmt
# x86_64-pc-windows-gnu cross-builds the Windows deployer. aarch64 musl lets the
# receiver gate run the appliance's NEON decoder under emulation on an x86 host,
# which is the only way that kernel gets exercised before it reaches a Pi.
RUST_TARGETS=(x86_64-pc-windows-gnu aarch64-unknown-linux-musl)
rustup target add --toolchain "${RUST_PIN}" "${RUST_TARGETS[@]}"

if ! command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
    install_windows_cross_toolchain
fi

# The pre-commit gate scans the tree and the runtime image with Trivy.
if ! command -v trivy >/dev/null 2>&1; then
    case "${OS}" in
        arch)
            sudo pacman -S --noconfirm trivy || "${SCRIPT_DIR}/install-trivy.sh"
            ;;
        darwin)
            brew install trivy
            ;;
        *)
            "${SCRIPT_DIR}/install-trivy.sh"
            ;;
    esac
fi

if [[ "$(uname -s)" == "Linux" ]]; then
    "${SCRIPT_DIR}/install-arm64-emulation.sh"
fi

echo ""
echo "=== Development Dependency Summary ==="
summary_tools=(cargo curl pkg-config python3 rustc rustfmt sha512sum shellcheck tar hadolint trivy)
if [[ "$(uname -s)" == "Linux" ]]; then
    # Podman or Docker runs the image gates; the mingw toolchain cross-builds
    # the Windows deployer that ships alongside the Linux one.
    summary_tools+=(podman x86_64-w64-mingw32-gcc x86_64-w64-mingw32-objdump)
fi
missing_tools=()
for tool in "${summary_tools[@]}"; do
    if command -v "${tool}" >/dev/null 2>&1; then
        echo -e "  ${tool}: ${GREEN}installed${NC}"
    else
        echo -e "  ${tool}: ${RED}missing${NC}"
        missing_tools+=("${tool}")
    fi
done

echo -e "  yamllint: ${GREEN}managed by tests/.venv${NC}"

# Clippy and rustfmt ship as Cargo subcommands, not as binaries on PATH: there
# has never been a `clippy` executable to find. Check what scripts/lint.sh
# actually runs, at the pinned toolchain, so the summary reflects the gate.
for subcommand in fmt clippy; do
    if (cd "${PROJECT_ROOT}" && cargo "${subcommand}" --version >/dev/null 2>&1); then
        echo -e "  cargo ${subcommand}: ${GREEN}installed${NC}"
    else
        echo -e "  cargo ${subcommand}: ${RED}missing${NC}"
        missing_tools+=("cargo-${subcommand}")
    fi
done

for target in "${RUST_TARGETS[@]}"; do
    if rustup target list --installed --toolchain "${RUST_PIN}" 2>/dev/null |
        grep -Fxq "${target}"; then
        echo -e "  rust target ${target}: ${GREEN}installed${NC}"
    else
        echo -e "  rust target ${target}: ${RED}missing${NC}"
        missing_tools+=("${target}")
    fi
done

# A distro Rust ahead of ~/.cargo/bin silently shadows the pin: every Cargo
# gate then aborts with "rustc N is not supported by the following packages"
# even though the pinned toolchain is installed. Resolve `cargo` the same way
# the gates do, from the project root, and report what it really is.
active_rust="$(cd "${PROJECT_ROOT}" && cargo --version 2>/dev/null | awk '{print $2}')"
if [[ "${active_rust}" == "${RUST_PIN}" ]]; then
    echo -e "  Native toolchain: ${GREEN}Rust ${active_rust} (edition 2024)${NC}"
else
    echo -e "  Native toolchain: ${RED}Rust ${active_rust:-unresolved}, expected ${RUST_PIN}${NC}"
    echo -e "${YELLOW}WARN${NC}: $(command -v cargo || echo cargo) does not honour rust-toolchain.toml."
    echo "      Put rustup ahead of any distribution Rust and re-open the shell:"
    echo "        . \"\${HOME}/.cargo/env\""
    echo "      A distribution rust/cargo package installed alongside rustup is"
    echo "      the usual cause and can be removed."
    missing_tools+=("rust-${RUST_PIN}")
fi

# Every gate runs on every commit, so a workstation missing a tool has to say
# so here rather than at the commit that cannot complete.
if ((${#missing_tools[@]} > 0)); then
    echo ""
    echo -e "${RED}ERROR${NC}: install did not provision: ${missing_tools[*]}" >&2
    exit 1
fi
