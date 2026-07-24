#!/bin/bash
# Central lint and syntax gate used by Make, local tests, and git hooks.

set -euo pipefail

ALLOW_MISSING=false
if [[ $# -gt 1 ]]; then
    echo "Usage: $0 [--allow-missing]" >&2
    exit 2
fi
case "${1:-}" in
    "") ;;
    --allow-missing) ALLOW_MISSING=true ;;
    -h|--help)
        echo "Usage: $0 [--allow-missing]"
        exit 0
        ;;
    *)
        echo "Usage: $0 [--allow-missing]" >&2
        exit 2
        ;;
esac

MISSING_TOOLS=()
missing_tool() {
    local tool="$1"
    if [[ "${ALLOW_MISSING}" == "true" ]]; then
        echo "SKIP: ${tool} not installed"
    else
        echo "ERROR: ${tool} is required (run make install)" >&2
        MISSING_TOOLS+=("${tool}")
    fi
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
PYTHON_VENV="${PROJECT_ROOT}/tests/.venv"

cd "${PROJECT_ROOT}"

mapfile -d '' -t SHELL_SCRIPTS < <(
    find "${PROJECT_ROOT}" \
        -path "${PROJECT_ROOT}/.git" -prune -o \
        -path "${PROJECT_ROOT}/.build" -prune -o \
        -path "${PROJECT_ROOT}/tests/.venv" -prune -o \
        -type f \( -name '*.sh' -o -path "${PROJECT_ROOT}/.githooks/*" \) \
        -print0
)

echo "Checking Bash syntax (${#SHELL_SCRIPTS[@]} scripts)..."
for script in "${SHELL_SCRIPTS[@]}"; do
    bash -n "${script}"
done

if command -v shellcheck >/dev/null 2>&1; then
    echo "Running ShellCheck..."
    shellcheck --severity=warning "${SHELL_SCRIPTS[@]}"
else
    missing_tool "shellcheck"
fi

if command -v hadolint >/dev/null 2>&1; then
    echo "Running Hadolint..."
    hadolint deploy/Dockerfile
else
    missing_tool "hadolint"
fi

if [[ -x "${PYTHON_VENV}/bin/python" ]] && \
   "${PYTHON_VENV}/bin/python" -c 'import yamllint' 2>/dev/null; then
    echo "Running yamllint..."
    "${PYTHON_VENV}/bin/python" -m yamllint -c .yamllint.yml \
        deploy/compose.yml docker-compose.dev.yml
else
    missing_tool "yamllint"
fi

if [[ -x "${PYTHON_VENV}/bin/python" ]] && \
   "${PYTHON_VENV}/bin/python" -c 'import ruff' 2>/dev/null; then
    echo "Running Ruff..."
    "${PYTHON_VENV}/bin/python" -m ruff check src scripts tools tests
else
    missing_tool "ruff"
fi

if [[ -x "${PYTHON_VENV}/bin/python" ]] && \
   "${PYTHON_VENV}/bin/python" -c 'import mypy' 2>/dev/null; then
    echo "Running mypy..."
    "${PYTHON_VENV}/bin/python" -m mypy src/omt_client
else
    missing_tool "mypy"
fi

if ((${#MISSING_TOOLS[@]} > 0)); then
    printf 'Missing required linters: %s\n' "${MISSING_TOOLS[*]}" >&2
    exit 1
fi

if [[ "${ALLOW_MISSING}" == "true" ]]; then
    echo "All available linters passed."
else
    echo "All required linters passed."
fi
