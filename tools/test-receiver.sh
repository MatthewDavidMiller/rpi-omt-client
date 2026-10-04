#!/bin/bash
# Test the C receiver, sender, and their preserved CLI contracts.
#
# The unit suites run under AddressSanitizer and UndefinedBehaviorSanitizer,
# which stand in for the memory safety the Rust receiver had from its
# compiler. The black-box suites then run against those same instrumented
# binaries, so a contract test that walks an out-of-bounds read fails here
# instead of passing quietly.
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${PROJECT_ROOT}"
command -v make >/dev/null 2>&1 || { echo "ERROR: make is required. Run: make install" >&2; exit 1; }

C_MAKE=(make -f mk/c.mk BUILD=asan -j"$(nproc)")
"${C_MAKE[@]}" receiver sender tests
"${C_MAKE[@]}" test
OUT="$("${C_MAKE[@]}" -s print-out)"
RECEIVER="${PROJECT_ROOT}/${OUT}/bin/omt-receiver"
SENDER="${PROJECT_ROOT}/${OUT}/bin/omt-test-sender"
export ASAN_OPTIONS="${ASAN_OPTIONS:-detect_leaks=1:abort_on_error=1}"
export UBSAN_OPTIONS="${UBSAN_OPTIONS:-print_stacktrace=1:halt_on_error=1}"

tests/native/test_receiver_cli.sh "${RECEIVER}"
tests/native/test_sender_receiver.sh "${SENDER}" "${RECEIVER}"
python3 tests/native/test_discovery_server.py "${RECEIVER}"
python3 tests/native/test_discovery_multi.py "${RECEIVER}"
# The D-Bus interop test needs dbus-python and PyGObject, which the system
# interpreter carries and the tooling virtualenv does not.
/usr/bin/python3 tests/native/test_mdns_dbus.py "${RECEIVER}"

# The appliance runs the AArch64 NEON kernels, which never execute on an x86
# workstation. Build and run the C suites in an emulated AArch64 Alpine
# container, so the kernel that ships is the one that was checked. Emulated
# timings mean nothing, but bit-exactness is exactly what needs proving here.
"${PROJECT_ROOT}/scripts/test-c-arm64.sh"
