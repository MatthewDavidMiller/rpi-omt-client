#!/bin/bash
# Run every libFuzzer target briefly from its committed corpus, so a parser
# regression the suites miss still fails a gate. FUZZ_SECONDS sets the time
# per target (default 30).
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec "${PROJECT_ROOT}/scripts/rust-sanitized.sh" fuzz "$@"
