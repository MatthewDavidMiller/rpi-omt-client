#!/bin/bash
# Test the Web frontend: its suites under AddressSanitizer, then the HTTPS
# contract and the container helpers against a sanitized binary.
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${PROJECT_ROOT}"
scripts/rust-sanitized.sh test -p omt-web
scripts/rust-sanitized.sh build -p omt-web --bin omt-web
WEB="${PROJECT_ROOT}/target/sanitized/x86_64-unknown-linux-musl/sanitize/omt-web"
export ASAN_OPTIONS="${ASAN_OPTIONS:-detect_leaks=1:abort_on_error=1}"
tests/native/test_web.sh "${WEB}"
REAL_OMT_WEB="${WEB}" tests/unit/test_entrypoint_logic.sh
