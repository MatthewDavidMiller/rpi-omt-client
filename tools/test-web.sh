#!/bin/bash
# Test the C Web frontend: its unit suites under ASan and UBSan, then the
# HTTPS contract and the container helpers against the instrumented binary.
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${PROJECT_ROOT}"
C_MAKE=(make -f mk/c.mk BUILD=asan -j"$(nproc)")
"${C_MAKE[@]}" web tests
"${PROJECT_ROOT}/$("${C_MAKE[@]}" -s print-out)/tests/test_web"
WEB="${PROJECT_ROOT}/$("${C_MAKE[@]}" -s print-out)/bin/omt-web"
export ASAN_OPTIONS="${ASAN_OPTIONS:-detect_leaks=1:abort_on_error=1}"
tests/native/test_web.sh "${WEB}"
REAL_OMT_WEB="${WEB}" tests/unit/test_entrypoint_logic.sh
