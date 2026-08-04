#!/bin/bash
# Build and test the native receiver with compiler hardening and sanitizers.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BUILD_DIR="${PROJECT_ROOT}/.build/native-receiver-clang-tests"

for tool in clang clang++ cmake ninja; do
    command -v "${tool}" >/dev/null 2>&1 || {
        echo "ERROR: ${tool} is required. Run: make install" >&2
        exit 1
    }
done

cmake -S "${PROJECT_ROOT}" -B "${BUILD_DIR}" -G Ninja \
    -DCMAKE_C_COMPILER=clang \
    -DCMAKE_CXX_COMPILER=clang++ \
    -DCMAKE_BUILD_TYPE=Debug \
    -DOMT_BUILD_RECEIVER=ON \
    -DOMT_BUILD_DEPLOYER=OFF \
    -DOMT_BUILD_TESTS=ON \
    -DOMT_ENABLE_SANITIZERS=ON
cmake --build "${BUILD_DIR}" --parallel 2
# LeakSanitizer cannot inspect processes under the workspace sandbox's ptrace
# policy. Address/UB checks remain active; CI may omit this override.
ASAN_OPTIONS="${ASAN_OPTIONS:-detect_leaks=0}" \
    ctest --test-dir "${BUILD_DIR}" --output-on-failure
