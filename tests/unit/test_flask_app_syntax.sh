#!/bin/bash
# Test Flask app syntax
# Run with: ./tests/unit/test_flask_app_syntax.sh

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo "Flask App Syntax Validation"
echo "============================"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
APP_PACKAGE="${PROJECT_ROOT}/src/omt_client"

if [[ ! -f "${APP_PACKAGE}/factory.py" || ! -f "${APP_PACKAGE}/wsgi.py" ]]; then
    echo -e "${RED}FAIL${NC}: omt_client application package not found"
    exit 1
fi

if ! command -v python3 &>/dev/null; then
    echo -e "${YELLOW}SKIP${NC}: Python 3 not available"
    exit 0
fi

echo "Checking Python syntax..."
if python3 -m compileall -q "${APP_PACKAGE}" 2>&1; then
    echo -e "${GREEN}OK${NC}: omt_client package syntax valid"
else
    echo -e "${RED}FAIL${NC}: omt_client package has syntax errors"
    exit 1
fi

echo "Checking required imports and routes..."
REQUIRED_PATTERNS=(
    "login_required"
    "create_app"
    "ServiceContainer"
    "def dashboard"
    "def select_source"
    "def restart_playback"
    "def clear_playback"
    "def network_settings"
    "def diagnostics"
    "def download_bundle"
    "CSRFProtect"
    "SESSION_COOKIE_SECURE"
    "security_headers"
    "Strict-Transport-Security"
)

for pattern in "${REQUIRED_PATTERNS[@]}"; do
    if grep -R -q "${pattern}" "${APP_PACKAGE}"; then
        echo -e "${GREEN}OK${NC}: found ${pattern}"
    else
        echo -e "${RED}FAIL${NC}: missing ${pattern}"
        exit 1
    fi
done

echo "============================"
echo -e "${GREEN}All Flask app syntax tests passed!${NC}"
