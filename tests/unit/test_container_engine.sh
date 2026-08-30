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

# A fake engine reports whatever FAKE_<NAME>_VERSION says, so a case can build
# the client the real world ships: podman-docker's /usr/bin/docker, which is
# named for Docker and answers as Podman. Left unset it prints nothing, which
# is the unrecognized-client case the helper falls back from.
make_fake_engine() {
    local bin_dir="$1"
    local engine_name="$2"

    mkdir -p "${bin_dir}"
    cat > "${bin_dir}/${engine_name}" <<'EOF'
#!/bin/bash
engine_name="$(basename "$0")"
printf '%s:%s\n' "${engine_name}" "$*" >> "${ENGINE_TEST_LOG}"
if [[ "${1:-}" == "--version" ]]; then
    if [[ "${engine_name}" == "docker" ]]; then
        [[ -n "${FAKE_DOCKER_VERSION:-}" ]] && printf '%s\n' "${FAKE_DOCKER_VERSION}"
    else
        [[ -n "${FAKE_PODMAN_VERSION:-}" ]] && printf '%s\n' "${FAKE_PODMAN_VERSION}"
    fi
    exit 0
fi
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

# The engine's own calls, with the client-identity probes filtered out. Cases
# assert on the operations the helper performs, so a probe landing before or
# after them must not move the line a case is reading.
engine_calls_without_probes() {
    grep -v -- '--version$' "$1"
}

# Make a bin directory the only thing on PATH for a case that needs an engine
# to be genuinely absent.
#
# Keeping /usr/bin on PATH would not do: whether Docker is "absent" would then
# depend on whether the machine running the gate happens to have a real
# /usr/bin/docker, and the toolbox image necessarily does -- it drives the host
# engine through its socket. What the helper and these cases' own assertions
# shell out to is linked in rather than inherited.
isolate_path() {
    local bin_dir="$1"
    local tool path
    for tool in basename grep sed wc; do
        path="$(command -v "${tool}")"
        ln -sf "${path}" "${bin_dir}/${tool}"
    done
    printf '%s\n' "${bin_dir}"
}

# Every case clears OMT_ENGINE_SERVER_KIND before it starts, and the two that
# are about it set it themselves. scripts/toolbox.sh exports it into the gate
# container to say what serves the mounted socket, so inside the toolbox it is
# always set -- and a case that inherited it would be answered before it could
# ask. The same reasoning is why CONTAINER_ENGINE is not forwarded at all;
# this variable has to be, so the cases neutralize it instead.
echo "=== Live Container Engine Selection Tests ==="

case_dir="${TEST_TMPDIR}/podman-only"
make_fake_engine "${case_dir}/bin" podman
if (
    unset CONTAINER_ENGINE CONTAINER_ENGINE_KIND CONTAINER_ENGINE_ANNOUNCED \
        OMT_ENGINE_SERVER_KIND
    export ENGINE_TEST_LOG="${case_dir}/calls"
    isolated_path="$(isolate_path "${case_dir}/bin")"
    export PATH="${isolated_path}"
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
    unset CONTAINER_ENGINE CONTAINER_ENGINE_KIND CONTAINER_ENGINE_ANNOUNCED \
        OMT_ENGINE_SERVER_KIND
    export ENGINE_TEST_LOG="${case_dir}/calls"
    export PATH="${case_dir}/bin:/usr/bin:/bin"
    # shellcheck disable=SC1090
    source "${HELPER}"
    export FAKE_DOCKER_VERSION="Docker version 29.5.2, build 79eb04c7"
    ensure_test_container_engine
    [[ "${CONTAINER_ENGINE_KIND}" == "docker" ]]
    container_engine_build -t test-image .
    [[ "$(engine_calls_without_probes "${case_dir}/calls" | sed -n '2p')" == \
       "docker:build -t test-image ." ]]
); then
    pass "A working Docker daemon remains preferred without Podman-only build flags"
else
    fail "A working Docker daemon should remain preferred"
fi

case_dir="${TEST_TMPDIR}/podman-fallback"
make_fake_engine "${case_dir}/bin" docker
make_fake_engine "${case_dir}/bin" podman
if (
    unset CONTAINER_ENGINE CONTAINER_ENGINE_KIND CONTAINER_ENGINE_ANNOUNCED \
        OMT_ENGINE_SERVER_KIND
    export ENGINE_TEST_LOG="${case_dir}/calls"
    export FAKE_DOCKER_INFO_STATUS=1
    export FAKE_PODMAN_INFO_STATUS=0
    export PATH="${case_dir}/bin:/usr/bin:/bin"
    # shellcheck disable=SC1090
    source "${HELPER}"
    ensure_test_container_engine
    [[ "${CONTAINER_ENGINE_KIND}" == "podman" ]]
    [[ "$(engine_calls_without_probes "${case_dir}/calls" | sed -n '1p')" == "docker:info" ]]
    [[ "$(engine_calls_without_probes "${case_dir}/calls" | sed -n '2p')" == "podman:info" ]]
    [[ "$(engine_calls_without_probes "${case_dir}/calls" | wc -l)" -eq 2 ]]
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
    unset CONTAINER_ENGINE_KIND CONTAINER_ENGINE_ANNOUNCED OMT_ENGINE_SERVER_KIND
    export PATH="${case_dir}/bin:/usr/bin:/bin"
    # shellcheck disable=SC1090
    source "${HELPER}"
    ensure_test_container_engine
    [[ "${CONTAINER_ENGINE_KIND}" == "podman" ]]
    container_engine_build -t test-image .
    [[ "$(engine_calls_without_probes "${case_dir}/calls" | sed -n '2p')" == \
       "podman:build --format docker -t test-image ." ]]
); then
    pass "CONTAINER_ENGINE selects Podman with Docker-format builds"
else
    fail "CONTAINER_ENGINE should explicitly select Podman"
fi

# podman-docker installs a /usr/bin/docker that execs Podman. Taking that name
# at face value picks Docker's spelling for every engine-specific decision the
# gates make -- the build format, the SELinux relabel, and the user mapping in
# scripts/toolbox.sh -- against a client that only understands Podman's.
case_dir="${TEST_TMPDIR}/podman-docker-shim"
make_fake_engine "${case_dir}/bin" docker
make_fake_engine "${case_dir}/bin" podman
if (
    unset CONTAINER_ENGINE CONTAINER_ENGINE_KIND CONTAINER_ENGINE_ANNOUNCED \
        OMT_ENGINE_SERVER_KIND
    export ENGINE_TEST_LOG="${case_dir}/calls"
    export FAKE_DOCKER_VERSION="podman version 6.1.0"
    export FAKE_PODMAN_VERSION="podman version 6.1.0"
    export PATH="${case_dir}/bin:/usr/bin:/bin"
    # shellcheck disable=SC1090
    source "${HELPER}"
    ensure_test_container_engine
    [[ "${CONTAINER_ENGINE_KIND}" == "podman" ]]
    # The real Podman binary, not the shim that would have to forward to it.
    [[ "${CONTAINER_ENGINE}" == "${case_dir}/bin/podman" ]]
    container_engine_build -t test-image .
    [[ "$(engine_calls_without_probes "${case_dir}/calls" | sed -n '2p')" == \
       "podman:build --format docker -t test-image ." ]]
); then
    pass "A docker-named Podman client is driven as Podman"
else
    fail "A docker-named Podman client should be driven as Podman"
fi

# The inverse, which the toolbox image itself is: a genuine Docker CLI whose
# server happens to be Podman. Only the client's spelling is in question here,
# so this one must stay Docker.
case_dir="${TEST_TMPDIR}/real-docker-cli"
make_fake_engine "${case_dir}/bin" docker
if (
    unset CONTAINER_ENGINE CONTAINER_ENGINE_KIND CONTAINER_ENGINE_ANNOUNCED \
        OMT_ENGINE_SERVER_KIND
    export ENGINE_TEST_LOG="${case_dir}/calls"
    export FAKE_DOCKER_VERSION="Docker version 29.5.2, build 79eb04c7"
    isolated_path="$(isolate_path "${case_dir}/bin")"
    export PATH="${isolated_path}"
    # shellcheck disable=SC1090
    source "${HELPER}"
    ensure_test_container_engine
    [[ "${CONTAINER_ENGINE_KIND}" == "docker" ]]
    container_engine_build -t test-image .
    [[ "$(engine_calls_without_probes "${case_dir}/calls" | sed -n '2p')" == \
       "docker:build -t test-image ." ]]
); then
    pass "A Docker CLI driving a Podman server keeps Docker's spelling"
else
    fail "A Docker CLI driving a Podman server should keep Docker's spelling"
fi

# Inside the toolbox both clients are installed and a Podman socket answers
# either of them, so the Docker CLI would win on first-found and every image
# built through it would lose its HEALTHCHECK. The engine serving the socket
# decides instead.
case_dir="${TEST_TMPDIR}/server-kind-podman"
make_fake_engine "${case_dir}/bin" docker
make_fake_engine "${case_dir}/bin" podman
if (
    unset CONTAINER_ENGINE CONTAINER_ENGINE_KIND CONTAINER_ENGINE_ANNOUNCED \
        OMT_ENGINE_SERVER_KIND
    export ENGINE_TEST_LOG="${case_dir}/calls"
    export OMT_ENGINE_SERVER_KIND=podman
    export FAKE_DOCKER_VERSION="Docker version 29.5.2, build 79eb04c7"
    export FAKE_PODMAN_VERSION="podman version 5.7.0"
    export PATH="${case_dir}/bin:/usr/bin:/bin"
    # shellcheck disable=SC1090
    source "${HELPER}"
    ensure_test_container_engine
    [[ "${CONTAINER_ENGINE_KIND}" == "podman" ]]
    [[ "${CONTAINER_ENGINE}" == "${case_dir}/bin/podman" ]]
    # The Docker CLI must not have been consulted at all: it answers this
    # socket too, so asking it first is what the case exists to prevent.
    ! grep -q '^docker:' "${case_dir}/calls"
    container_engine_build -t test-image .
    [[ "$(engine_calls_without_probes "${case_dir}/calls" | sed -n '2p')" == \
       "podman:build --format docker -t test-image ." ]]
); then
    pass "A Podman server is driven by the Podman client even where Docker answers"
else
    fail "A Podman server should be driven by the Podman client"
fi

# The same signal must not strand a Docker server on a Podman client.
case_dir="${TEST_TMPDIR}/server-kind-docker"
make_fake_engine "${case_dir}/bin" docker
make_fake_engine "${case_dir}/bin" podman
if (
    unset CONTAINER_ENGINE CONTAINER_ENGINE_KIND CONTAINER_ENGINE_ANNOUNCED \
        OMT_ENGINE_SERVER_KIND
    export ENGINE_TEST_LOG="${case_dir}/calls"
    export OMT_ENGINE_SERVER_KIND=docker
    export FAKE_DOCKER_VERSION="Docker version 29.5.2, build 79eb04c7"
    export FAKE_PODMAN_VERSION="podman version 5.7.0"
    export PATH="${case_dir}/bin:/usr/bin:/bin"
    # shellcheck disable=SC1090
    source "${HELPER}"
    ensure_test_container_engine
    [[ "${CONTAINER_ENGINE_KIND}" == "docker" ]]
    container_engine_build -t test-image .
    [[ "$(engine_calls_without_probes "${case_dir}/calls" | sed -n '2p')" == \
       "docker:build -t test-image ." ]]
); then
    pass "A Docker server keeps the Docker client"
else
    fail "A Docker server should keep the Docker client"
fi

# An engine that answers nothing recognizable is no worse off than before the
# helper started asking: the installed name decides.
case_dir="${TEST_TMPDIR}/unknown-version"
make_fake_engine "${case_dir}/bin" docker
if (
    unset CONTAINER_ENGINE CONTAINER_ENGINE_KIND CONTAINER_ENGINE_ANNOUNCED \
        OMT_ENGINE_SERVER_KIND
    export ENGINE_TEST_LOG="${case_dir}/calls"
    isolated_path="$(isolate_path "${case_dir}/bin")"
    export PATH="${isolated_path}"
    # shellcheck disable=SC1090
    source "${HELPER}"
    ensure_test_container_engine
    [[ "${CONTAINER_ENGINE_KIND}" == "docker" ]]
); then
    pass "An unrecognized client falls back to the name it is installed under"
else
    fail "An unrecognized client should fall back to its installed name"
fi

case_dir="${TEST_TMPDIR}/mounts"
mkdir -p "${case_dir}/host"
if (
    unset CONTAINER_ENGINE CONTAINER_ENGINE_KIND CONTAINER_ENGINE_ANNOUNCED \
        OMT_ENGINE_SERVER_KIND
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
    unset CONTAINER_ENGINE_KIND CONTAINER_ENGINE_ANNOUNCED OMT_ENGINE_SERVER_KIND
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
