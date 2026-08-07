#!/bin/bash
# Test the Rust receiver crates and their preserved CLI contract.
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${PROJECT_ROOT}"
command -v cargo >/dev/null 2>&1 || { echo "ERROR: cargo is required. Run: make install" >&2; exit 1; }
cargo test --locked -p omt-protocol -p vmx-decoder -p omt-receiver-core -p omt-receiver
cargo build --locked -p omt-receiver
tests/native/test_receiver_cli.sh "${PROJECT_ROOT}/target/debug/omt-receiver"
python3 tests/native/test_discovery_server.py "${PROJECT_ROOT}/target/debug/omt-receiver"
python3 tests/native/test_discovery_multi.py "${PROJECT_ROOT}/target/debug/omt-receiver"

# The appliance runs the AArch64 NEON inverse DCT, which never executes on an
# x86 workstation. Cross-build the decoder and run its conformance vectors and
# its scalar-versus-NEON differential under emulation, so the kernel that ships
# is the one that was checked. Emulated timings mean nothing, but bit-exactness
# is exactly what needs proving here.
TARGET=aarch64-unknown-linux-musl
if rustup target list --installed 2>/dev/null | grep -Fxq "${TARGET}" &&
    command -v qemu-aarch64-static >/dev/null 2>&1; then
    echo "Checking the AArch64 NEON decoder under emulation..."
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_RUNNER=qemu-aarch64-static \
    RUSTFLAGS="-Clink-self-contained=yes" \
        cargo test --locked --release -p vmx-decoder --target "${TARGET}"
else
    echo "ERROR: the ${TARGET} Rust target and qemu-aarch64-static are required" >&2
    echo "       to check the NEON decoder. Run: make install" >&2
    exit 1
fi
