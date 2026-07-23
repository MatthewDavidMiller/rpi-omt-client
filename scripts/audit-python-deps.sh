#!/bin/bash
# Audit Python requirement files for known vulnerabilities.
# Usage: ./scripts/audit-python-deps.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

PIP_AUDIT_BIN="${PIP_AUDIT_BIN:-}"
PIP_AUDIT_CACHE_DIR="${PIP_AUDIT_CACHE_DIR:-${PROJECT_ROOT}/.build/pip-audit-cache}"

if [[ -z "${PIP_AUDIT_BIN}" ]]; then
    if [[ -x "${PROJECT_ROOT}/tests/.venv/bin/pip-audit" ]]; then
        PIP_AUDIT_BIN="${PROJECT_ROOT}/tests/.venv/bin/pip-audit"
    else
        PIP_AUDIT_BIN="pip-audit"
    fi
fi

if ! command -v "${PIP_AUDIT_BIN}" >/dev/null 2>&1; then
    echo "FAIL: pip-audit is required for Python dependency auditing (${PIP_AUDIT_BIN} not found)" >&2
    echo "Run: make test-setup" >&2
    exit 1
fi

mkdir -p "${PIP_AUDIT_CACHE_DIR}"
cd "${PROJECT_ROOT}"

audit_hash_locked() {
    local req_file="$1"
    echo "=== pip-audit: ${req_file} ==="
    "${PIP_AUDIT_BIN}" \
        --require-hashes \
        --disable-pip \
        --strict \
        --cache-dir "${PIP_AUDIT_CACHE_DIR}" \
        --progress-spinner off \
        -r "${req_file}"
}

audit_pinned_no_deps() {
    local req_file="$1"
    echo "=== pip-audit: ${req_file} ==="
    "${PIP_AUDIT_BIN}" \
        --no-deps \
        --disable-pip \
        --strict \
        --cache-dir "${PIP_AUDIT_CACHE_DIR}" \
        --progress-spinner off \
        -r "${req_file}"
}

failures=0

run_audit() {
    local audit_function="$1"
    local req_file="$2"
    if ! "${audit_function}" "${req_file}"; then
        echo "FAIL: dependency audit reported findings for ${req_file}" >&2
        failures=$((failures + 1))
    fi
    echo ""
}

run_audit audit_hash_locked "app/requirements.txt"
run_audit audit_pinned_no_deps "tests/requirements-dev.txt"

if (( failures > 0 )); then
    echo "Dependency audit failed for ${failures} lock file(s)." >&2
    exit 1
fi

echo "All Python dependency audits passed."
