#!/bin/bash
# Build and run the C unit suites on AArch64 under emulation.
#
# The appliance's NEON kernels only compile for AArch64, and QEMU models no
# Raspberry Pi SoC, so this proves the kernels agree with the portable ones
# bit for bit and decode the conformance vectors exactly -- not that they are
# fast. It runs the same Alpine release the appliance image is built on.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
# shellcheck source=scripts/docker-test-env.sh
source "${SCRIPT_DIR}/docker-test-env.sh"

ALPINE_IMAGE="docker.io/library/alpine:3.23.5@$(sed -n 's/^ARG ALPINE_DIGEST=//p' "${PROJECT_ROOT}/deploy/Dockerfile")"
BUILD="${BUILD:-release}"

if ! ensure_test_container_engine; then
    echo "ERROR: Docker or Podman is required for the AArch64 C suites" >&2
    exit 1
fi

"${CONTAINER_ENGINE}" run --rm --platform linux/arm64 \
    -v "${PROJECT_ROOT}:/work:Z" -w /work \
    "${ALPINE_IMAGE}" sh -euc "
        apk add --no-cache alsa-lib-dev build-base openssl-dev linux-headers python3 >/dev/null
        make -f mk/c.mk BUILD=${BUILD} OMT_VERSION=test -j\$(nproc) tests
        make -f mk/c.mk BUILD=${BUILD} OMT_VERSION=test test
    "
