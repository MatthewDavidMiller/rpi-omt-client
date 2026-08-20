#!/bin/bash
# Test the Rust receiver, sender, and their preserved CLI contracts.
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${PROJECT_ROOT}"
command -v cargo >/dev/null 2>&1 || { echo "ERROR: cargo is required. Run: make install" >&2; exit 1; }
cargo test --locked -p omt-protocol -p vmx-decoder -p omt-receiver-core -p omt-receiver \
    -p omt-test-sender
cargo build --locked -p omt-receiver -p omt-test-sender
tests/native/test_receiver_cli.sh "${PROJECT_ROOT}/target/debug/omt-receiver"
tests/native/test_sender_receiver.sh \
    "${PROJECT_ROOT}/target/debug/omt-test-sender" \
    "${PROJECT_ROOT}/target/debug/omt-receiver"
python3 tests/native/test_discovery_server.py "${PROJECT_ROOT}/target/debug/omt-receiver"
python3 tests/native/test_discovery_multi.py "${PROJECT_ROOT}/target/debug/omt-receiver"

# The appliance runs the AArch64 NEON inverse DCT, which never executes on an
# x86 workstation. Cross-build the decoder and run its conformance vectors and
# its scalar-versus-NEON differential under emulation, so the kernel that ships
# is the one that was checked. Emulated timings mean nothing, but bit-exactness
# is exactly what needs proving here.
TARGET=aarch64-unknown-linux-musl
# Debian and Fedora name the user-mode emulator qemu-aarch64-static; Alpine,
# which is what the toolbox image is built on, ships it as qemu-aarch64. Either
# runs the decoder, so either satisfies this.
QEMU_AARCH64=""
for candidate in qemu-aarch64-static qemu-aarch64; do
    if command -v "${candidate}" >/dev/null 2>&1; then
        QEMU_AARCH64="${candidate}"
        break
    fi
done
if rustup target list --installed 2>/dev/null | grep -Fxq "${TARGET}" &&
    [[ -n "${QEMU_AARCH64}" ]]; then
    echo "Checking the AArch64 NEON decoder under emulation with ${QEMU_AARCH64}..."
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_RUNNER="${QEMU_AARCH64}" \
    RUSTFLAGS="-Clink-self-contained=yes" \
        cargo test --locked --release -p vmx-decoder --target "${TARGET}"
else
    echo "ERROR: the ${TARGET} Rust target and one of qemu-aarch64-static or" >&2
    echo "       qemu-aarch64 are required to check the NEON decoder." >&2
    echo "       Run: make install" >&2
    exit 1
fi
