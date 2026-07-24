#!/bin/bash
# Exercise native OMT discovery and direct-receiver network paths without
# requiring a vendor sender or Raspberry Pi display hardware.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
# shellcheck source=scripts/docker-test-env.sh
source "${PROJECT_ROOT}/scripts/docker-test-env.sh"

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'
IMAGE_TAG="omt-client:network-test"
NETWORK_NAME="omt-client-network-test-$$"

cleanup() {
    if [[ -n "${CONTAINER_ENGINE:-}" ]]; then
        "${CONTAINER_ENGINE}" network rm -f "${NETWORK_NAME}" >/dev/null 2>&1 || true
        "${CONTAINER_ENGINE}" rmi "${IMAGE_TAG}" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

pass() { printf "${GREEN}PASS${NC}: %s\n" "$1"; }
fail() {
    printf "${RED}FAIL${NC}: %s\n" "$1" >&2
    exit 1
}

echo "Native OMT Network Integration Test"
echo "==================================="

# shellcheck disable=SC2310
ensure_test_container_engine || fail "Docker or Podman is required"
cd "${PROJECT_ROOT}"
# shellcheck disable=SC2310
container_engine_build \
    -f deploy/Dockerfile \
    --build-arg RPI_OMT_CLIENT_VERSION=vtest \
    -t "${IMAGE_TAG}" . || fail "container image build failed"
"${CONTAINER_ENGINE}" network create --driver bridge "${NETWORK_NAME}" >/dev/null ||
    fail "isolated bridge creation failed"

discovery="$(
    timeout 15 "${CONTAINER_ENGINE}" run --rm \
        --network "${NETWORK_NAME}" \
        --entrypoint /usr/local/bin/omt-receiver \
        "${IMAGE_TAG}" discover --wait-ms 250 --json
)" || fail "native OMT discovery failed on an isolated bridge"
if printf '%s' "${discovery}" | python3 -c \
    'import json,sys; assert isinstance(json.load(sys.stdin), list)'; then
    pass "native discovery returns a JSON source list on a non-loopback network"
else
    fail "native discovery output is not a JSON source list"
fi

probe_file="$(mktemp)"
set +e
timeout 15 "${CONTAINER_ENGINE}" run --rm \
    --network "${NETWORK_NAME}" \
    --entrypoint /usr/local/bin/omt-receiver \
    "${IMAGE_TAG}" probe \
    --target omt://127.0.0.1:65000 --timeout-ms 250 --json >"${probe_file}"
probe_status=$?
set -e
if [[ "${probe_status}" -ne 3 ]]; then
    rm -f "${probe_file}"
    fail "unreachable direct OMT target did not return the documented no-media status"
fi
if python3 - "${probe_file}" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as stream:
    result = json.load(stream)
assert result["ok"] is False
assert result["target"] == "omt://127.0.0.1:65000"
assert result["video"] is False
assert result["audio"] is False
assert isinstance(result["error"], str)
PY
then
    pass "direct-target probe returns bounded structured no-media diagnostics"
else
    rm -f "${probe_file}"
    fail "direct-target probe output is malformed"
fi
rm -f "${probe_file}"

set +e
invalid_output="$(
    timeout 15 "${CONTAINER_ENGINE}" run --rm \
        --network "${NETWORK_NAME}" \
        --entrypoint /usr/local/bin/omt-receiver \
        "${IMAGE_TAG}" probe \
        --target 'omt://127.0.0.1:65000/path' --timeout-ms 250 --json 2>&1
)"
invalid_status=$?
set -e
if [[ "${invalid_status}" -eq 2 ]] &&
   grep -Fq "Invalid OMT direct target" <<<"${invalid_output}"; then
    pass "direct-target validation rejects paths before connecting"
else
    fail "invalid direct target was not rejected"
fi

echo "==================================="
echo -e "${GREEN}Native OMT network integration tests passed!${NC}"
