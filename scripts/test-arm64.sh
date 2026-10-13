#!/bin/bash
# Build the appliance crates' suites for AArch64 and run them under emulation.
#
# The appliance's NEON kernels only assemble for AArch64, and QEMU models no
# Raspberry Pi SoC, so this proves the kernels agree with the portable ones
# bit for bit and decode the conformance vectors exactly -- not that they are
# fast.
#
# The suites are cross-compiled with the linker and flags the appliance image
# is built with (clang and lld against the aarch64 sysroot the toolbox installs
# from the appliance's Alpine release; see scripts/make-sysroot.sh), and only
# the test binaries run under qemu-user. Extra arguments go to cargo test.
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${PROJECT_ROOT}"

SYSROOT="${OMT_AARCH64_SYSROOT:-/opt/sysroot-aarch64}"

for tool in cargo clang ld.lld qemu-aarch64; do
    command -v "${tool}" >/dev/null 2>&1 || {
        echo "ERROR: ${tool} is required for the AArch64 suites (run make install)" >&2
        exit 1
    }
done
[[ -e "${SYSROOT}/lib/ld-musl-aarch64.so.1" ]] || {
    echo "ERROR: no AArch64 sysroot at ${SYSROOT} (run make install)" >&2
    exit 1
}

export OMT_SYSROOT="${SYSROOT}"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_RUNNER="qemu-aarch64 -L ${SYSROOT}"
# Release: the decode suites are minutes under emulation in a debug build.
cargo test --release --offline --locked --target aarch64-unknown-linux-musl \
    -p omt-common -p omt-crypto -p omt-protocol -p omt-vmx -p omt-receiver-core \
    -p omt-receiver "$@"
