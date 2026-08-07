#!/bin/bash
# Verify repo-local Python tools survive checkout relocation and fail clearly.

set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"

if grep -ERn --include='Makefile' --include='*.sh' \
    '(tests/\.venv|PYTEST_VENV)/bin/(pytest|pip|ruff|mypy|yamllint|pip-audit)' \
    "${ROOT}/Makefile" "${ROOT}/scripts" "${ROOT}/tools" >/dev/null; then
    echo "FAIL: Python tooling invokes a generated virtualenv console script" >&2
    exit 1
fi
grep -Fq '$(TEST_PYTHON) -m pytest' "${ROOT}/Makefile"
grep -Fq '$(TEST_PYTHON) -m pip install' "${ROOT}/Makefile"
grep -Fq '"${PYTHON_VENV}" -m pip_audit' "${ROOT}/scripts/audit-python-deps.sh"

CASE_DIR="$(mktemp -d)"
trap 'rm -rf "${CASE_DIR}"' EXIT
FIXTURE="${CASE_DIR}/moved-checkout"
OUTSIDE="${CASE_DIR}/outside"
mkdir -p \
    "${FIXTURE}/scripts" \
    "${FIXTURE}/src/omt_client" \
    "${FIXTURE}/deploy" \
    "${FIXTURE}/tests/.venv/bin" \
    "${FIXTURE}/fake-bin" \
    "${OUTSIDE}"
cp "${ROOT}/scripts/lint.sh" "${FIXTURE}/scripts/lint.sh"
touch \
    "${FIXTURE}/deploy/Dockerfile" \
    "${FIXTURE}/deploy/compose.yml" \
    "${FIXTURE}/docker-compose.dev.yml" \
    "${FIXTURE}/.yamllint.yml" \
    "${FIXTURE}/src/omt_client/__init__.py"

for command_name in cargo shellcheck hadolint; do
    cat > "${FIXTURE}/fake-bin/${command_name}" <<'EOF'
#!/bin/bash
exit 0
EOF
    chmod +x "${FIXTURE}/fake-bin/${command_name}"
done

cat > "${FIXTURE}/tests/.venv/bin/python" <<'EOF'
#!/bin/bash
set -euo pipefail
if [[ "${1:-}" == "-c" ]]; then
    [[ "${PYTHON_MODULES_AVAILABLE:-1}" == "1" ]]
    exit
fi
[[ "${1:-}" == "-m" ]]
printf '%s\n' "${2:-}" >> "${PYTHON_MODULE_LOG}"
EOF
chmod +x "${FIXTURE}/tests/.venv/bin/python"

(
    cd "${OUTSIDE}"
    PATH="${FIXTURE}/fake-bin:${PATH}" \
        PYTHON_MODULE_LOG="${CASE_DIR}/module.log" \
        "${FIXTURE}/scripts/lint.sh" >/dev/null
)
# ruff runs twice (check, then format --check) and mypy runs twice (strict over
# src/ and scripts/, then relaxed over tests/).
printf 'yamllint\nruff\nruff\nmypy\nmypy\n' > "${CASE_DIR}/expected.log"
cmp "${CASE_DIR}/expected.log" "${CASE_DIR}/module.log"

if (
    cd "${OUTSIDE}"
    PATH="${FIXTURE}/fake-bin:${PATH}" \
        PYTHON_MODULES_AVAILABLE=0 \
        PYTHON_MODULE_LOG="${CASE_DIR}/missing.log" \
        "${FIXTURE}/scripts/lint.sh" >/dev/null 2>&1
); then
    echo "FAIL: lint accepted missing repo-local Python modules" >&2
    exit 1
fi
# No flag turns a missing linter into a pass: an option that did would make the
# lint gate report on whichever tools happened to be installed.
if (
    cd "${OUTSIDE}"
    PATH="${FIXTURE}/fake-bin:${PATH}" \
        PYTHON_MODULES_AVAILABLE=0 \
        PYTHON_MODULE_LOG="${CASE_DIR}/missing.log" \
        "${FIXTURE}/scripts/lint.sh" --allow-missing >/dev/null 2>&1
); then
    echo "FAIL: lint still offers a mode that tolerates missing linters" >&2
    exit 1
fi

echo "Python tooling relocation and missing-tool tests passed"
