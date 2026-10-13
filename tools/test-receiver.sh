#!/bin/bash
# Test the receiver, the test sender, and their preserved CLI contracts.
#
# The unit suites run under AddressSanitizer, which checks the unsafe code the
# compiler cannot: the foreign calls and the decoder's kernels. The black-box
# suites then run against sanitized binaries too, so a contract test that walks
# an out-of-bounds read in an unsafe block fails here instead of passing
# quietly.
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${PROJECT_ROOT}"
command -v cargo >/dev/null 2>&1 || { echo "ERROR: cargo is required. Run: make install" >&2; exit 1; }

scripts/rust-sanitized.sh test -p omt-common -p omt-crypto -p omt-protocol -p omt-vmx \
    -p omt-receiver-core -p omt-receiver -p omt-test-sender
scripts/rust-sanitized.sh build -p omt-receiver --bin omt-receiver -p omt-test-sender --bin omt-test-sender
BIN="${PROJECT_ROOT}/target/sanitized/x86_64-unknown-linux-musl/sanitize"
RECEIVER="${BIN}/omt-receiver"
SENDER="${BIN}/omt-test-sender"
export ASAN_OPTIONS="${ASAN_OPTIONS:-detect_leaks=1:abort_on_error=1}"

tests/native/test_receiver_cli.sh "${RECEIVER}"
tests/native/test_sender_receiver.sh "${SENDER}" "${RECEIVER}"
python3 tests/native/test_discovery_server.py "${RECEIVER}"
python3 tests/native/test_discovery_multi.py "${RECEIVER}"
# The D-Bus interop test needs dbus-python and PyGObject, which the system
# interpreter carries and the tooling virtualenv does not.
/usr/bin/python3 tests/native/test_mdns_dbus.py "${RECEIVER}"

# The appliance runs the AArch64 NEON kernels, which never execute on an x86
# workstation. Build and run the suites for AArch64 under emulation, so the
# kernel that ships is the one that was checked. Emulated timings mean
# nothing, but bit-exactness is exactly what needs proving here.
"${PROJECT_ROOT}/scripts/test-arm64.sh"
