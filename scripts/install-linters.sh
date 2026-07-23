#!/bin/bash
# Install linters for CI/CD
# Usage: ./scripts/install-linters.sh
#
# Installs:
#   - ShellCheck: Shell script static analysis
#   - Hadolint: Dockerfile linter
#   - yamllint: YAML linter
#   - Trivy: filesystem and image security scanning
#   - pip-audit: Python package vulnerability auditing

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
PYTEST_VENV="${PROJECT_ROOT}/tests/.venv"

echo "=== Installing Linters ==="
echo ""

# Detect OS
if [[ -f /etc/os-release ]]; then
    . /etc/os-release
    OS=${ID}
else
    # shellcheck disable=SC2312
    OS=$(uname -s | tr '[:upper:]' '[:lower:]')
fi

echo "Detected OS: ${OS}"
echo ""

# Install ShellCheck
echo "Installing ShellCheck..."
if command -v shellcheck &> /dev/null; then
    # shellcheck disable=SC2312
    echo -e "${GREEN}OK${NC}: ShellCheck already installed ($(shellcheck --version | head -2 | tail -1))"
else
    case "${OS}" in
        ubuntu|debian)
            sudo apt-get update && sudo apt-get install -y shellcheck
            ;;
        fedora)
            sudo dnf install -y ShellCheck
            ;;
        arch)
            sudo pacman -S --noconfirm shellcheck
            ;;
        darwin)
            brew install shellcheck
            ;;
        *)
            echo -e "${YELLOW}WARN${NC}: Unknown OS. Install ShellCheck manually: https://github.com/koalaman/shellcheck#installing"
            ;;
    esac

    if command -v shellcheck &> /dev/null; then
        echo -e "${GREEN}OK${NC}: ShellCheck installed"
    fi
fi
echo ""

# Install Hadolint
echo "Installing Hadolint..."
if command -v hadolint &> /dev/null; then
    # shellcheck disable=SC2312
    echo -e "${GREEN}OK${NC}: Hadolint already installed ($(hadolint --version))"
else
    case "${OS}" in
        ubuntu|debian)
            "${SCRIPT_DIR}/install-hadolint.sh"
            ;;
        fedora)
            sudo dnf install -y hadolint
            ;;
        arch)
            sudo pacman -S --noconfirm hadolint
            ;;
        darwin)
            brew install hadolint
            ;;
        *)
            echo -e "${YELLOW}WARN${NC}: Unknown OS. Install Hadolint manually: https://github.com/hadolint/hadolint#install"
            ;;
    esac

    if command -v hadolint &> /dev/null; then
        echo -e "${GREEN}OK${NC}: Hadolint installed"
    fi
fi
echo ""

# Install yamllint into the test tooling venv
echo "Installing yamllint..."
if [[ -x "${PYTEST_VENV}/bin/yamllint" ]]; then
    # shellcheck disable=SC2312
    echo -e "${GREEN}OK${NC}: yamllint already installed ($("${PYTEST_VENV}/bin/yamllint" --version))"
else
    if [[ ! -x "${PYTEST_VENV}/bin/pip" ]]; then
        python3 -m venv "${PYTEST_VENV}"
    fi
    "${PYTEST_VENV}/bin/pip" install -r "${PROJECT_ROOT}/tests/requirements-dev.txt"
    if [[ -x "${PYTEST_VENV}/bin/yamllint" ]]; then
        echo -e "${GREEN}OK${NC}: yamllint installed"
    fi
fi
echo ""

# Install Trivy
TRIVY_VERSION="0.69.3"
echo "Installing Trivy..."
if command -v trivy &> /dev/null; then
    # shellcheck disable=SC2312
    echo -e "${GREEN}OK${NC}: Trivy already installed ($(trivy --version | head -1))"
else
    case "${OS}" in
        ubuntu|debian)
            TRIVY_DOWNLOAD_DIR=$(mktemp -d)
            TRIVY_ARCHIVE="trivy_${TRIVY_VERSION}_Linux-64bit.tar.gz"
            TRIVY_CHECKSUMS="trivy_${TRIVY_VERSION}_checksums.txt"
            curl -fsSL "https://github.com/aquasecurity/trivy/releases/download/v${TRIVY_VERSION}/${TRIVY_ARCHIVE}" -o "${TRIVY_DOWNLOAD_DIR}/${TRIVY_ARCHIVE}"
            curl -fsSL "https://github.com/aquasecurity/trivy/releases/download/v${TRIVY_VERSION}/${TRIVY_CHECKSUMS}" -o "${TRIVY_DOWNLOAD_DIR}/${TRIVY_CHECKSUMS}"
            # shellcheck disable=SC2312
            grep " ${TRIVY_ARCHIVE}\$" "${TRIVY_DOWNLOAD_DIR}/${TRIVY_CHECKSUMS}" | \
                (cd "${TRIVY_DOWNLOAD_DIR}" && sha256sum -c -)
            tar -xzf "${TRIVY_DOWNLOAD_DIR}/${TRIVY_ARCHIVE}" \
                -C "${TRIVY_DOWNLOAD_DIR}" trivy
            sudo install -m 0755 "${TRIVY_DOWNLOAD_DIR}/trivy" /usr/local/bin/trivy
            rm -rf "${TRIVY_DOWNLOAD_DIR}"
            ;;
        fedora)
            sudo dnf install -y trivy
            ;;
        arch)
            sudo pacman -S --noconfirm trivy
            ;;
        darwin)
            brew install trivy
            ;;
        *)
            echo -e "${YELLOW}WARN${NC}: Unknown OS. Install Trivy manually: https://trivy.dev/latest/getting-started/installation/"
            ;;
    esac

    if command -v trivy &> /dev/null; then
        echo -e "${GREEN}OK${NC}: Trivy installed"
    fi
fi
echo ""

# Install pip-audit into the test tooling venv
echo "Installing pip-audit..."
if [[ -x "${PYTEST_VENV}/bin/pip-audit" ]]; then
    # shellcheck disable=SC2312
    echo -e "${GREEN}OK${NC}: pip-audit already installed ($("${PYTEST_VENV}/bin/pip-audit" --version))"
else
    if [[ ! -x "${PYTEST_VENV}/bin/pip" ]]; then
        python3 -m venv "${PYTEST_VENV}"
    fi
    "${PYTEST_VENV}/bin/pip" install -r "${PROJECT_ROOT}/tests/requirements-dev.txt"
    if [[ -x "${PYTEST_VENV}/bin/pip-audit" ]]; then
        echo -e "${GREEN}OK${NC}: pip-audit installed"
    fi
fi
echo ""

# Summary
echo "=== Linter Installation Summary ==="
echo -n "  ShellCheck: "
if command -v shellcheck &> /dev/null; then
    echo -e "${GREEN}installed${NC}"
else
    echo -e "${RED}not installed${NC}"
fi

echo -n "  Hadolint:   "
if command -v hadolint &> /dev/null; then
    echo -e "${GREEN}installed${NC}"
else
    echo -e "${RED}not installed${NC}"
fi

echo -n "  yamllint:   "
if [[ -x "${PROJECT_ROOT}/tests/.venv/bin/yamllint" ]]; then
    echo -e "${GREEN}installed${NC}"
else
    echo -e "${RED}not installed${NC}"
fi

echo -n "  Trivy:      "
if command -v trivy &> /dev/null; then
    echo -e "${GREEN}installed${NC}"
else
    echo -e "${RED}not installed${NC}"
fi

echo -n "  pip-audit:  "
if [[ -x "${PROJECT_ROOT}/tests/.venv/bin/pip-audit" ]]; then
    echo -e "${GREEN}installed${NC}"
else
    echo -e "${RED}not installed${NC}"
fi

echo ""
echo "Run tests with: ./scripts/test-local.sh"
echo "Run security scans with: ./scripts/security-scan.sh"
