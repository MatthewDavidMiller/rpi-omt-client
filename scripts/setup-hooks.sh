#!/bin/bash
# Setup git hooks for local CI/CD
# Usage: ./scripts/setup-hooks.sh

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
HOOKS_DIR="${PROJECT_ROOT}/.githooks"

echo "=== Setting up Git Hooks ==="

# Ensure we're in a git repo
if ! git -C "${PROJECT_ROOT}" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "ERROR: Not a git repository"
    exit 1
fi

# Keep the tracked hook live so updates cannot drift from copied .git files.
[[ -f "${HOOKS_DIR}/pre-commit" ]] || {
    echo "ERROR: Missing tracked hook: ${HOOKS_DIR}/pre-commit" >&2
    exit 1
}
chmod +x "${HOOKS_DIR}/pre-commit"
git -C "${PROJECT_ROOT}" config --local core.hooksPath .githooks
echo "Configured core.hooksPath=.githooks"

echo ""
echo "Git hooks installed successfully!"
echo ""
echo "Hooks will run automatically:"
echo "  - pre-commit: Full tests, audits, Windows publish, and security scans"
echo ""
echo "To bypass hooks temporarily (not recommended):"
echo "  git commit --no-verify"
