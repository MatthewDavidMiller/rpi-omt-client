#!/bin/bash
# Run blocking security scans for the repo and container image.
# Usage: ./scripts/security-scan.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
# shellcheck source=scripts/docker-test-env.sh
source "${SCRIPT_DIR}/docker-test-env.sh"

TRIVY_BIN="${TRIVY_BIN:-trivy}"
SCAN_IMAGE_TAG="${SECURITY_SCAN_IMAGE_TAG:-omt-client:security-scan}"
SEVERITY_LEVELS="${TRIVY_SEVERITY_LEVELS:-HIGH,CRITICAL}"

if ! command -v "${TRIVY_BIN}" >/dev/null 2>&1; then
    echo "FAIL: Trivy is required for security scans (${TRIVY_BIN} not found)" >&2
    exit 1
fi

# shellcheck disable=SC2310
if ! ensure_test_container_engine; then
    echo "FAIL: Docker or Podman is required for image security scans" >&2
    exit 1
fi

cd "${PROJECT_ROOT}"

echo "=== Trivy filesystem scan ==="
"${TRIVY_BIN}" fs \
    --scanners vuln,secret,misconfig \
    --ignore-unfixed \
    --severity "${SEVERITY_LEVELS}" \
    --exit-code 1 \
    --no-progress \
    --skip-dirs tests/.venv \
    --skip-dirs .git \
    --skip-dirs .build \
    --skip-dirs dist \
    --skip-dirs deployer/RpiOmt.Deployer.App/bin \
    --skip-dirs deployer/RpiOmt.Deployer.App/obj \
    --skip-dirs deployer/RpiOmt.Deployer.Core/bin \
    --skip-dirs deployer/RpiOmt.Deployer.Core/obj \
    --skip-dirs deployer/RpiOmt.Deployer.Tests/bin \
    --skip-dirs deployer/RpiOmt.Deployer.Tests/obj \
    --skip-dirs deployer/RpiOmt.Deployer.IntegrationTests/bin \
    --skip-dirs deployer/RpiOmt.Deployer.IntegrationTests/obj \
    --skip-dirs build \
    --skip-dirs output \
    --skip-dirs work \
    --skip-dirs env \
    --skip-dirs vm-files \
    --skip-dirs pi-gen \
    --skip-dirs .codex \
    --skip-files vars.yml \
    --skip-files .env \
    .

echo "=== Building image for security scan ==="
container_engine_build --build-arg RPI_OMT_CLIENT_VERSION="vtest" -t "${SCAN_IMAGE_TAG}" .

SCAN_IMAGE_ARCHIVE="$(mktemp)"
trap 'rm -f "${SCAN_IMAGE_ARCHIVE}"' EXIT
if [[ "${CONTAINER_ENGINE_KIND}" == "podman" ]]; then
    "${CONTAINER_ENGINE}" save --format docker-archive -o "${SCAN_IMAGE_ARCHIVE}" "${SCAN_IMAGE_TAG}"
else
    "${CONTAINER_ENGINE}" save -o "${SCAN_IMAGE_ARCHIVE}" "${SCAN_IMAGE_TAG}"
fi

echo "=== Trivy image scan ==="
"${TRIVY_BIN}" image \
    --scanners vuln,secret \
    --ignore-unfixed \
    --severity "${SEVERITY_LEVELS}" \
    --exit-code 1 \
    --no-progress \
    --input "${SCAN_IMAGE_ARCHIVE}"
