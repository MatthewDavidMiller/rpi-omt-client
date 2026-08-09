#!/bin/bash
# Local test runner
# Usage: ./scripts/test-local.sh [--quick|--full]
#
# Options:
#   --quick   Run unit tests only (no container engine required, ~30 sec)
#   --full    Run unit tests + image build + container smoke + OMT network tests
#   (default) Run unit tests + image build (no smoke tests)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
# shellcheck source=scripts/docker-test-env.sh
source "${SCRIPT_DIR}/docker-test-env.sh"
cd "${PROJECT_ROOT}"

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'
PYTEST_VENV="${PROJECT_ROOT}/tests/.venv"
PYTHON_TEST_BIN="${PYTEST_VENV}/bin/python"
if [[ ! -x "${PYTHON_TEST_BIN}" ]]; then
    echo -e "${RED}FAILED${NC}: Python test venv not set up. Run: make test-setup"
    exit 1
fi
# Shell integration tests execute application commands such as Werkzeug's
# password helper. Keep all repo-local Python tools resolvable after a checkout
# is moved without relying on generated console-script shebangs.
export PATH="${PYTEST_VENV}/bin:${PATH}"

echo "=== RPi OMT Client Local Test Runner ==="
echo "Project root: ${PROJECT_ROOT}"
echo ""

QUICK_MODE=false
FULL_MODE=false
if [[ $# -gt 1 ]]; then
    echo "Usage: $0 [--quick|--full]" >&2
    exit 2
fi
case "${1:-}" in
    "") ;;
    --quick)
        QUICK_MODE=true
        echo "Running in quick mode (unit tests only, no container engine)"
        ;;
    --full)
        FULL_MODE=true
        echo "Running in full mode (unit + image build + container smoke + OMT network tests)"
        ;;
    -h|--help)
        echo "Usage: $0 [--quick|--full]"
        exit 0
        ;;
    *)
        echo "Usage: $0 [--quick|--full]" >&2
        exit 2
        ;;
esac

# run_test LABEL COMMAND [ARG...]
#
# Every gate reports through here so the run's output names which suites
# actually executed. A gate invoked directly would still abort the run under
# `set -e`, but silently -- indistinguishable from one that was skipped.
run_test() {
    local label="$1"
    shift
    echo "=== ${label} ==="
    if "$@"; then
        echo -e "${GREEN}PASSED${NC}: ${label}"
    else
        echo -e "${RED}FAILED${NC}: ${label}"
        exit 1
    fi
    echo ""
}

# ─── Unit Tests ───────────────────────────────────────────────
run_test "Flask App Syntax" "${PROJECT_ROOT}/tests/unit/test_flask_app_syntax.sh"
run_test "OMT Controller" "${PROJECT_ROOT}/tests/unit/test_control_omt.sh"
run_test "Entrypoint Logic" "${PROJECT_ROOT}/tests/unit/test_entrypoint_logic.sh"
run_test "Start OMT Script" "${PROJECT_ROOT}/tests/unit/test_start_omt.sh"
run_test "Host Diagnostics" "${PROJECT_ROOT}/tests/unit/test_host_diagnostics.sh"
run_test "Host Event Watcher" "${PROJECT_ROOT}/tests/unit/test_host_event_watcher.sh"
run_test "Host Reboot Bridge" "${PROJECT_ROOT}/tests/unit/test_host_reboot.sh"
run_test "Host Reboot Behavior" "${PROJECT_ROOT}/tests/unit/test_host_reboot_behavior.sh"
run_test "Host Install Helpers" "${PROJECT_ROOT}/tests/unit/test_host_install_helpers.sh"
run_test "Board Profiles" "${PROJECT_ROOT}/tests/unit/test_board_profile.sh"
run_test "HDMI Boot Configuration" "${PROJECT_ROOT}/tests/unit/test_hdmi_config.sh"
run_test "Deployment Transactions" "${PROJECT_ROOT}/tests/unit/test_deployment_transactions.sh"
run_test "Compose Config" "${PROJECT_ROOT}/tests/unit/test_compose_config.sh"
run_test "Alpine Bootstrap" "${PROJECT_ROOT}/tests/unit/test_bootstrap.sh"
run_test "Firewall Reachability" "${PROJECT_ROOT}/tests/unit/test_firewall_reachability.sh"
run_test "Install Script" "${PROJECT_ROOT}/tests/unit/test_install.sh"
run_test "OpenRC Services" "${PROJECT_ROOT}/tests/unit/test_openrc_services.sh"
run_test "ARM64 Artifact" "${PROJECT_ROOT}/tests/unit/test_build_artifact.sh"
run_test "Uninstall Script" "${PROJECT_ROOT}/tests/unit/test_uninstall.sh"
run_test "Version Detection" "${PROJECT_ROOT}/tests/unit/test_detect_version.sh"
run_test "Container Engine Selection" "${PROJECT_ROOT}/tests/unit/test_container_engine.sh"
run_test "Git Hook Setup" "${PROJECT_ROOT}/tests/unit/test_setup_hooks.sh"
run_test "Python Tooling" "${PROJECT_ROOT}/tests/unit/test_python_tooling.sh"
run_test "Test Runner Arguments" "${PROJECT_ROOT}/tests/unit/test_test_runner_args.sh"
run_test "Supply Chain Guardrails" "${PROJECT_ROOT}/tests/unit/test_supply_chain.sh"
run_test "Lint and syntax" "${PROJECT_ROOT}/scripts/lint.sh"
run_test "Receiver Core" "${PROJECT_ROOT}/tools/test-receiver.sh"

# ─── Rust Deployer Tests ─────────────────────────────────────
if [[ "${QUICK_MODE}" == "true" ]]; then
    run_test "Deployer Core and CLI" "${PROJECT_ROOT}/scripts/check-deployer.sh"
else
    run_test "Deployer Core, CLI, and GUI publish" \
        "${PROJECT_ROOT}/scripts/check-deployer.sh" --publish
    # Both published deployer packages come off this one Linux workstation, so
    # the Windows package is built and header-verified in the same gate that
    # builds the host one.
    run_test "Windows Deployer Cross Build" "${PROJECT_ROOT}/scripts/build-windows-deployer.sh"
fi

# ─── Python Unit Tests ────────────────────────────────────────
echo "=== Python Unit Tests ==="
if "${PYTHON_TEST_BIN}" -m pytest tests/unit \
    --cov=src/omt_client --cov-report=term-missing --cov-fail-under=100 -q --tb=short; then
    echo -e "${GREEN}PASSED${NC}: Python unit tests"
else
    echo -e "${RED}FAILED${NC}: Python unit tests"
    exit 1
fi
echo ""

if [[ "${QUICK_MODE}" == "true" ]]; then
    echo -e "${GREEN}=== Quick tests completed successfully ===${NC}"
    exit 0
fi

# ─── Live Container Tests ─────────────────────────────────────
# shellcheck disable=SC2310
if ! ensure_test_container_engine; then
    echo -e "${RED}FAILED${NC}: Docker or Podman is required for live container tests"
    exit 1
fi

run_test "Dockerfile lint" "${PROJECT_ROOT}/tests/unit/test_dockerfile_lint.sh"
# Builds the appliance's own ARM64 receiver stage as well as the amd64 image.
# The gate itself requires registered emulation in every mode; there is no
# argument or variable here that can reduce what it checks.
run_test "Container Image Build" "${PROJECT_ROOT}/tests/integration/test_docker_build.sh"

if [[ "${FULL_MODE}" == "true" ]]; then
    run_test "Container Smoke Tests" "${PROJECT_ROOT}/tests/integration/test_container_smoke.sh"
    run_test "OMT Network Tests" "${PROJECT_ROOT}/tests/integration/test_omt_network.sh"
fi

echo -e "${GREEN}=== All tests completed successfully ===${NC}"
