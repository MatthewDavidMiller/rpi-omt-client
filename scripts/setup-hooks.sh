#!/bin/bash
# Setup git hooks for local CI/CD
# Usage: ./scripts/setup-hooks.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
HOOKS_DIR="${PROJECT_ROOT}/.githooks"

echo "=== Setting up Git Hooks ==="

# Ensure we're in a git repo
if ! git -C "${PROJECT_ROOT}" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "ERROR: Not a git repository"
    exit 1
fi

# Keep the tracked hooks live so updates cannot drift from copied .git files.
for hook in pre-commit post-commit; do
    [[ -f "${HOOKS_DIR}/${hook}" ]] || {
        echo "ERROR: Missing tracked hook: ${HOOKS_DIR}/${hook}" >&2
        exit 1
    }
    chmod +x "${HOOKS_DIR}/${hook}"
done
git -C "${PROJECT_ROOT}" config --local core.hooksPath .githooks
echo "Configured core.hooksPath=.githooks"

echo ""
echo "Git hooks installed successfully!"
echo ""
echo "Hooks will run automatically:"
echo "  - pre-commit:  Full tests, audits, cross builds, and security scans"
echo "  - post-commit: ARM64 image and both deployer packages, versioned to"
echo "                 the commit that was just made"
echo ""
echo "To bypass the pre-commit gate temporarily (not recommended):"
echo "  git commit --no-verify"
echo ""
echo "--no-verify does not skip post-commit; the builds still run. The same"
echo "builds by hand:"
echo "  make build-arm64 build-deployer build-windows-deployer"
