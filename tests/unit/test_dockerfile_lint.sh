#!/bin/bash
# Lint the Dockerfile with hadolint
# Run with: ./tests/unit/test_dockerfile_lint.sh

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo "Dockerfile Lint Check"
echo "====================="

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
DOCKERFILE="${PROJECT_ROOT}/Dockerfile"

if [[ ! -f "${DOCKERFILE}" ]]; then
    echo -e "${RED}FAIL${NC}: Dockerfile not found"
    exit 1
fi

if ! command -v hadolint &>/dev/null; then
    echo -e "${YELLOW}SKIP${NC}: hadolint not installed"
    echo "  Install with: brew install hadolint  OR  apt-get install hadolint"
    exit 0
fi

echo "Running hadolint..."
if hadolint "${DOCKERFILE}"; then
    echo -e "${GREEN}PASS${NC}: Dockerfile passed hadolint"
else
    echo -e "${RED}FAIL${NC}: Dockerfile failed hadolint"
    exit 1
fi

echo "====================="
echo -e "${GREEN}Dockerfile lint passed!${NC}"
