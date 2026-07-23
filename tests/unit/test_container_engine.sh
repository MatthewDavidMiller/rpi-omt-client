#!/bin/bash
# Unit tests for Docker/Podman selection used by live container tests.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
HELPER="${PROJECT_ROOT}/scripts/docker-test-env.sh"

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

PASS=0
FAIL=0

pass() { echo -e "${GREEN}PASS${NC}: $1"; PASS=$((PASS + 1)); }
fail() { echo -e "${RED}FAIL${NC}: $1"; FAIL=$((FAIL + 1)); }

TEST_TMPDIR="$(mktemp -d)"
trap 'rm -rf "${TEST_TMPDIR}"' EXIT

make_fake_engine() {
    local bin_dir="$1"
    local engine_name="$2"

    mkdir -p "${bin_dir}"
    cat > "${bin_dir}/${engine_name}" <<'EOF'
#!/bin/bash
engine_name="$(basename "$0")"
printf '%s:%s\n' "${engine_name}" "$*" >> "${ENGINE_TEST_LOG}"
if [[ "${1:-}" == "info" ]]; then
    if [[ "${engine_name}" == "docker" ]]; then
        exit "${FAKE_DOCKER_INFO_STATUS:-0}"
    fi
    exit "${FAKE_PODMAN_INFO_STATUS:-0}"
fi
exit 0
EOF
    chmod +x "${bin_dir}/${engine_name}"
}

echo "=== Live Container Engine Selection Tests ==="

case_dir="${TEST_TMPDIR}/podman-only"
make_fake_engine "${case_dir}/bin" podman
if (
    unset CONTAINER_ENGINE CONTAINER_ENGINE_KIND CONTAINER_ENGINE_ANNOUNCED
    export ENGINE_TEST_LOG="${case_dir}/calls"
    export PATH="${case_dir}/bin:/usr/bin:/bin"
    # shellcheck disable=SC1090
    source "${HELPER}"
    ensure_test_container_engine
    [[ "${CONTAINER_ENGINE_KIND}" == "podman" ]]
    [[ "${CONTAINER_ENGINE}" == "${case_dir}/bin/podman" ]]
); then
    pass "Podman is selected when Docker is absent"
else
    fail "Podman should be selected when Docker is absent"
fi

case_dir="${TEST_TMPDIR}/docker-first"
make_fake_engine "${case_dir}/bin" docker
make_fake_engine "${case_dir}/bin" podman
if (
    unset CONTAINER_ENGINE CONTAINER_ENGINE_KIND CONTAINER_ENGINE_ANNOUNCED
    export ENGINE_TEST_LOG="${case_dir}/calls"
    export PATH="${case_dir}/bin:/usr/bin:/bin"
    # shellcheck disable=SC1090
    source "${HELPER}"
    ensure_test_container_engine
    [[ "${CONTAINER_ENGINE_KIND}" == "docker" ]]
    container_engine_build -t test-image .
    [[ "$(sed -n '2p' "${case_dir}/calls")" == "docker:build -t test-image ." ]]
); then
    pass "A working Docker daemon remains preferred without Podman-only build flags"
else
    fail "A working Docker daemon should remain preferred"
fi

case_dir="${TEST_TMPDIR}/podman-fallback"
make_fake_engine "${case_dir}/bin" docker
make_fake_engine "${case_dir}/bin" podman
if (
    unset CONTAINER_ENGINE CONTAINER_ENGINE_KIND CONTAINER_ENGINE_ANNOUNCED
    export ENGINE_TEST_LOG="${case_dir}/calls"
    export FAKE_DOCKER_INFO_STATUS=1
    export FAKE_PODMAN_INFO_STATUS=0
    export PATH="${case_dir}/bin:/usr/bin:/bin"
    # shellcheck disable=SC1090
    source "${HELPER}"
    ensure_test_container_engine
    [[ "${CONTAINER_ENGINE_KIND}" == "podman" ]]
    [[ "$(sed -n '1p' "${case_dir}/calls")" == "docker:info" ]]
    [[ "$(sed -n '2p' "${case_dir}/calls")" == "podman:info" ]]
    [[ "$(wc -l < "${case_dir}/calls")" -eq 2 ]]
); then
    pass "Podman is used when an installed Docker daemon is unavailable"
else
    fail "Podman should be used before attempting to start an unavailable Docker daemon"
fi

case_dir="${TEST_TMPDIR}/explicit"
make_fake_engine "${case_dir}/bin" docker
make_fake_engine "${case_dir}/bin" podman
if (
    export ENGINE_TEST_LOG="${case_dir}/calls"
    export CONTAINER_ENGINE=podman
    unset CONTAINER_ENGINE_KIND CONTAINER_ENGINE_ANNOUNCED
    export PATH="${case_dir}/bin:/usr/bin:/bin"
    # shellcheck disable=SC1090
    source "${HELPER}"
    ensure_test_container_engine
    [[ "${CONTAINER_ENGINE_KIND}" == "podman" ]]
    container_engine_build -t test-image .
    [[ "$(sed -n '2p' "${case_dir}/calls")" == \
       "podman:build --format docker -t test-image ." ]]
); then
    pass "CONTAINER_ENGINE selects Podman with Docker-format builds"
else
    fail "CONTAINER_ENGINE should explicitly select Podman"
fi

case_dir="${TEST_TMPDIR}/mounts"
mkdir -p "${case_dir}/host"
if (
    unset CONTAINER_ENGINE CONTAINER_ENGINE_KIND CONTAINER_ENGINE_ANNOUNCED
    # shellcheck disable=SC1090
    source "${HELPER}"
    CONTAINER_ENGINE_KIND=podman
    [[ "$(container_engine_volume "${case_dir}/host" /container/path ro)" == \
       "${case_dir}/host:/container/path:ro,Z" ]]
    CONTAINER_ENGINE_KIND=docker
    [[ "$(container_engine_volume "${case_dir}/host" /container/path ro)" == \
       "${case_dir}/host:/container/path:ro" ]]
); then
    pass "Podman bind mounts receive SELinux relabeling without changing Docker mounts"
else
    fail "Container bind mount options should be engine-specific"
fi

case_dir="${TEST_TMPDIR}/invalid"
mkdir -p "${case_dir}/bin"
cp /bin/true "${case_dir}/bin/nerdctl"
if (
    export CONTAINER_ENGINE="${case_dir}/bin/nerdctl"
    unset CONTAINER_ENGINE_KIND CONTAINER_ENGINE_ANNOUNCED
    # shellcheck disable=SC1090
    source "${HELPER}"
    ! ensure_test_container_engine >/dev/null 2>&1
); then
    pass "Unsupported explicit container engines fail closed"
else
    fail "Unsupported explicit container engines should fail closed"
fi

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[[ "${FAIL}" -eq 0 ]]
