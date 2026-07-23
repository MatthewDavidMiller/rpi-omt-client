#!/bin/bash
# Verify local hook setup references the tracked hooks rather than stale copies.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
TEST_DIR="$(mktemp -d)"
trap 'rm -rf "${TEST_DIR}"' EXIT

mkdir -p "${TEST_DIR}/scripts" "${TEST_DIR}/.githooks"
cp "${PROJECT_ROOT}/scripts/setup-hooks.sh" "${TEST_DIR}/scripts/setup-hooks.sh"
cp "${PROJECT_ROOT}/.githooks/pre-commit" "${TEST_DIR}/.githooks/pre-commit"
git -C "${TEST_DIR}" init -q

"${TEST_DIR}/scripts/setup-hooks.sh" >/dev/null

[[ "$(git -C "${TEST_DIR}" config --local core.hooksPath)" == ".githooks" ]]
[[ -x "${TEST_DIR}/.githooks/pre-commit" ]]
[[ ! -e "${TEST_DIR}/.git/hooks/pre-commit" ]]

echo "PASS: tracked hooks are configured through core.hooksPath"
