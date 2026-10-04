#!/bin/bash
# Build and stage the first-party C OMT test sender.
#
#   --target auto     this workstation (the default)
#   --target aarch64  a Raspberry Pi 4 or 5, built in the same emulated Alpine
#                     the appliance image is built on, so the binary links the
#                     same musl the Pi runs
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BUILD_HOME="${OMT_TEST_SENDER_HOME:-${PROJECT_ROOT}/.build/omt-test-sender}"

usage() {
    echo "Usage: $0 [--target auto|aarch64]" >&2
}

requested_target="auto"
if [[ $# -eq 2 && "$1" == "--target" ]]; then
    requested_target="$2"
elif [[ $# -ne 0 ]]; then
    usage
    exit 2
fi
case "${requested_target}" in
    auto | aarch64) ;;
    *) usage; exit 2 ;;
esac

command -v make >/dev/null 2>&1 || { echo "ERROR: make is required. Run: make install" >&2; exit 1; }
host_target="$(cc -dumpmachine)"
cd "${PROJECT_ROOT}"

if [[ "${requested_target}" == "auto" ]]; then
    echo "Building the C OMT test sender for ${host_target}..."
    make -s -f mk/c.mk BUILD=release OMT_VERSION=sender -j"$(nproc)" sender
    out="$(make -s -f mk/c.mk BUILD=release OMT_VERSION=sender print-out)"
    target="${host_target}"
else
    # shellcheck source=scripts/docker-test-env.sh
    source "${SCRIPT_DIR}/docker-test-env.sh"
    ensure_test_container_engine || { echo "ERROR: Docker or Podman is required for the ARM64 sender" >&2; exit 1; }
    image="docker.io/library/alpine:3.23.5@$(sed -n 's/^ARG ALPINE_DIGEST=//p' deploy/Dockerfile)"
    echo "Building the C OMT test sender for aarch64 under emulation..."
    "${CONTAINER_ENGINE}" run --rm --platform linux/arm64 \
        -v "${PROJECT_ROOT}:/work:Z" -w /work "${image}" sh -euc '
            apk add --no-cache build-base linux-headers >/dev/null
            make -s -f mk/c.mk BUILD=release OMT_VERSION=sender LDFLAGS=-static-pie -j$(nproc) sender
        '
    out="build/c/aarch64-alpine-linux-musl/release-cc"
    target="aarch64-alpine-linux-musl"
fi

source_binary="${PROJECT_ROOT}/${out#"${PROJECT_ROOT}/"}/bin/omt-test-sender"
[[ -x "${source_binary}" ]] || {
    echo "ERROR: sender build did not produce ${source_binary}" >&2
    exit 1
}

artifact_dir="${BUILD_HOME}/artifacts/${target}"
mkdir -p "${artifact_dir}/bin"
install -m 0755 "${source_binary}" "${artifact_dir}/bin/omt-test-sender"
printf '%s\n' "target=${target}" "source=src/sender" > "${artifact_dir}/BUILD-INFO"

if [[ "${target}" == "${host_target}" ]]; then
    current_tmp="${BUILD_HOME}/.current.$$"
    ln -s "artifacts/${target}" "${current_tmp}"
    mv -Tf "${current_tmp}" "${BUILD_HOME}/current"
    echo "Built and activated: ${artifact_dir}"
else
    echo "Built cross-target artifact (not activated on ${host_target}): ${artifact_dir}"
fi
