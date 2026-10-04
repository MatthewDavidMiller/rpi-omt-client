#!/bin/bash
# Static gates for the C tree. Together with the sanitizer suites in
# tools/test-receiver.sh, these stand in for what the Rust compiler enforced:
#
#   * a -Werror build at every optimization level and with both compilers,
#     because GCC and Clang each warn about things the other does not;
#   * GCC's -fanalyzer over every translation unit, for leaks, double frees,
#     use-after-free, and NULL dereferences along error paths;
#   * cppcheck, for the bounds and lifetime mistakes neither compiler flags;
#   * clang-format, so a review diff is only ever about behaviour;
#   * the generated Unicode tables, regenerated and compared.
#
# banned.h makes every poisoned libc call a compile error, so the builds below
# also prove none of them crept back in.
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${PROJECT_ROOT}"

missing=()
for tool in gcc clang clang-format cppcheck make python3; do
    command -v "${tool}" >/dev/null 2>&1 || missing+=("${tool}")
done
if ((${#missing[@]})); then
    echo "ERROR: C gate tools are missing: ${missing[*]} (run make install)" >&2
    exit 1
fi

mapfile -d '' -t C_FILES < <(
    find src tests/c tests/fuzz -type f \( -name '*.c' -o -name '*.h' \) \
        ! -name nfc_tables.c ! -name templates_gen.c ! -name blowfish_tables.c \
        -print0 2>/dev/null | sort -z
)

echo "Checking C formatting (${#C_FILES[@]} files)..."
clang-format --dry-run -Werror "${C_FILES[@]}"

echo "Checking for unseeded libc randomness..."
if grep -nE '\b(s?rand)\s*\(' "${C_FILES[@]}"; then
    echo "ERROR: use omt_random_bytes, not rand()/srand()" >&2
    exit 1
fi

echo "Checking generated sources..."
python3 tools/gen/gen_nfc_tables.py --check
python3 tools/gen/gen_templates.py --check
python3 tools/gen/gen_blowfish_tables.py --check

jobs="$(nproc)"
for compiler in gcc clang; do
    for build in debug release; do
        echo "Building with ${compiler} (${build}) under -Werror..."
        make -s -f mk/c.mk CC="${compiler}" BUILD="${build}" OMT_VERSION=check -j"${jobs}" \
            all tests deploy-libs deploy-tests
    done
done

echo "Running the GCC static analyzer..."
make -s -f mk/c.mk CC=gcc BUILD=analyze OMT_VERSION=check -j"${jobs}" \
    all tests deploy-libs deploy-tests

# The Windows deployer's own sources only compile for Windows, so the cross
# compiler holds them to the same -Werror.
if command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
    echo "Building the deployer for Windows under -Werror..."
    make -s -f mk/c.mk CC=x86_64-w64-mingw32-gcc BUILD=debug OMT_VERSION=check \
        OPENSSL_PREFIX="$(scripts/build-openssl.sh x86_64-w64-mingw32)" -j"${jobs}" deploy-libs
fi

echo "Running cppcheck..."
cppcheck --quiet --error-exitcode=1 --std=c11 --inline-suppr \
    --enable=warning,portability \
    -D_GNU_SOURCE -DOMT_VERSION='"check"' -Isrc \
    --suppress=missingIncludeSystem \
    --suppress=normalCheckLevelMaxBranches \
    src tests/c

echo "C static gates passed."
