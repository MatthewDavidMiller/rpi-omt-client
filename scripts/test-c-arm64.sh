#!/bin/bash
# Build the C unit suites for AArch64 and run them under emulation.
#
# The appliance's NEON kernels only compile for AArch64, and QEMU models no
# Raspberry Pi SoC, so this proves the kernels agree with the portable ones
# bit for bit and decode the conformance vectors exactly -- not that they are
# fast.
#
# The suites are cross-compiled with the compiler and flags the appliance
# image is built with (clang, against the aarch64 sysroot the toolbox installs
# from the appliance's Alpine release; see scripts/make-sysroot.sh), and only
# the test binaries run under qemu-user. Compiling under emulation instead
# took minutes per run and tested GCC's code while clang's shipped. The link
# flags are deploy/Dockerfile's, for the reason given there.

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${PROJECT_ROOT}"

SYSROOT="${OMT_AARCH64_SYSROOT:-/opt/sysroot-aarch64}"
BUILD="${BUILD:-release}"

for tool in clang ld.lld qemu-aarch64 make; do
    command -v "${tool}" >/dev/null 2>&1 || {
        echo "ERROR: ${tool} is required for the AArch64 C suites (run make install)" >&2
        exit 1
    }
done
[[ -e "${SYSROOT}/lib/ld-musl-aarch64.so.1" ]] || {
    echo "ERROR: no AArch64 sysroot at ${SYSROOT} (run make install)" >&2
    exit 1
}

C_MAKE=(make -f mk/c.mk
    CC="clang --target=aarch64-alpine-linux-musl --sysroot=${SYSROOT}"
    LDFLAGS="-fuse-ld=lld -static-libgcc" BUILD="${BUILD}" OMT_VERSION=test -j"$(nproc)")
"${C_MAKE[@]}" tests
"${C_MAKE[@]}" TEST_RUNNER="qemu-aarch64 -L ${SYSROOT}" test
