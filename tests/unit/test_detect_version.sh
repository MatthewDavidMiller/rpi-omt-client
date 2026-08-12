#!/bin/bash
# Unit tests for scripts/detect-version.sh.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
DETECT_VERSION="${PROJECT_ROOT}/scripts/detect-version.sh"

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

PASS=0
FAIL=0

pass() { echo -e "${GREEN}PASS${NC}: $1"; PASS=$((PASS + 1)); }
fail() { echo -e "${RED}FAIL${NC}: $1"; FAIL=$((FAIL + 1)); }

assert_equals() {
    local expected="$1"
    local actual="$2"
    local label="$3"
    if [[ "${actual}" == "${expected}" ]]; then
        pass "${label}"
    else
        fail "${label}: expected '${expected}', got '${actual}'"
    fi
}

echo "=== Version Detection Tests ==="

tmpdir="$(mktemp -d)"
trap 'rm -rf "${tmpdir}"' EXIT

mkdir -p "${tmpdir}/rpi-omt-client-v0.1"
actual="$(RPI_OMT_CLIENT_VERSION=v9.9.9 "${DETECT_VERSION}" "${tmpdir}/rpi-omt-client-v0.1")"
assert_equals "v9.9.9" \
    "${actual}" \
    "Explicit RPI_OMT_CLIENT_VERSION wins"

actual="$(GITHUB_REF_TYPE=tag GITHUB_REF_NAME=v1.2.3 "${DETECT_VERSION}" "${tmpdir}/rpi-omt-client-v0.1")"
assert_equals "v0.1" \
    "${actual}" \
    "GitHub environment is ignored in favor of release-directory names"

actual="$("${DETECT_VERSION}" "${tmpdir}/rpi-omt-client-v0.1")"
assert_equals "v0.1" \
    "${actual}" \
    "Release directory with a v-prefixed version is detected"

mkdir -p "${tmpdir}/rpi-omt-client-0.2.0"
actual="$("${DETECT_VERSION}" "${tmpdir}/rpi-omt-client-0.2.0")"
assert_equals "0.2.0" \
    "${actual}" \
    "Release directory without a v-prefix is detected"

mkdir -p "${tmpdir}/rpi-omt-client-main"
printf '%s\n' \
    '[workspace]' \
    '' \
    '[workspace.package]' \
    'version = "4.5.6"' >"${tmpdir}/rpi-omt-client-main/Cargo.toml"
git -C "${tmpdir}/rpi-omt-client-main" init --quiet
git -C "${tmpdir}/rpi-omt-client-main" \
    -c user.name="Version Test" \
    -c user.email="version-test@example.invalid" \
    commit --allow-empty --quiet --message="test"
git -C "${tmpdir}/rpi-omt-client-main" tag v4.5.5
actual="$("${DETECT_VERSION}" "${tmpdir}/rpi-omt-client-main")"
assert_equals "v4.5.6" \
    "${actual}" \
    "Canonical project version wins over an existing tag"

mkdir -p "${tmpdir}/rpi-omt-client-main-no-version"
actual="$("${DETECT_VERSION}" "${tmpdir}/rpi-omt-client-main-no-version")"
assert_equals "unknown" \
    "${actual}" \
    "Non-release archive directory falls back to unknown"

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[[ "${FAIL}" -eq 0 ]]
