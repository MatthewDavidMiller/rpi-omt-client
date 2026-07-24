#!/bin/bash
# Audit Python requirement files for known vulnerabilities.
# Usage: ./scripts/audit-python-deps.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

PIP_AUDIT_BIN="${PIP_AUDIT_BIN:-}"
PIP_AUDIT_CACHE_DIR="${PIP_AUDIT_CACHE_DIR:-${PROJECT_ROOT}/.build/pip-audit-cache}"
PYTHON_VENV="${PROJECT_ROOT}/tests/.venv/bin/python"

if [[ -z "${PIP_AUDIT_BIN}" ]]; then
    if [[ -x "${PYTHON_VENV}" ]] && \
       "${PYTHON_VENV}" -c 'import pip_audit' 2>/dev/null; then
        PIP_AUDIT_COMMAND=("${PYTHON_VENV}" -m pip_audit)
    else
        PIP_AUDIT_COMMAND=(python3 -m pip_audit)
    fi
else
    PIP_AUDIT_COMMAND=("${PIP_AUDIT_BIN}")
fi

if ! "${PIP_AUDIT_COMMAND[@]}" --version >/dev/null 2>&1; then
    echo "FAIL: pip-audit is required for Python dependency auditing" >&2
    echo "Run: make test-setup" >&2
    exit 1
fi

mkdir -p "${PIP_AUDIT_CACHE_DIR}"
cd "${PROJECT_ROOT}"

audit_hash_locked() {
    local req_file="$1"
    echo "=== pip-audit: ${req_file} ==="
    "${PIP_AUDIT_COMMAND[@]}" \
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
    "${PIP_AUDIT_COMMAND[@]}" \
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

run_audit audit_hash_locked "requirements/runtime.txt"
run_audit audit_pinned_no_deps "tests/requirements-dev.txt"

if (( failures > 0 )); then
    echo "Dependency audit failed for ${failures} lock file(s)." >&2
    exit 1
fi

echo "All Python dependency audits passed."
