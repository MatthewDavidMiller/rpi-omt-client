#!/bin/bash
# The Rust gates that need the pinned nightly toolchain:
#
#   ./scripts/rust-sanitized.sh test [cargo test args...]
#       every suite under AddressSanitizer, or the packages named with -p
#   ./scripts/rust-sanitized.sh build [cargo build args...]
#       the named binaries, instrumented the same way, into
#       target/sanitized/x86_64-unknown-linux-musl/sanitize
#   ./scripts/rust-sanitized.sh fuzz [target...]
#       every libFuzzer target for FUZZ_SECONDS (default 30) from its
#       committed corpus
#
# Rust ships sanitizer runtimes for glibc targets only, so the standard
# library is rebuilt for x86_64-unknown-linux-musl (-Zbuild-std) and linked
# against Alpine's own compiler-rt. The C library is linked dynamically,
# which ASan requires.
# Nothing built here is ever published.
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${PROJECT_ROOT}"

NIGHTLY="$(sed -n 's/^RUST_NIGHTLY="\(.*\)"$/\1/p' scripts/install-rust.sh)"
TARGET=x86_64-unknown-linux-musl
CLANG_RT="$(clang -print-resource-dir)/lib/linux"
BASE_FLAGS="-Zsanitizer=address -Ctarget-feature=-crt-static -Cforce-frame-pointers=yes"
# Rust's own ASan runtime is linked automatically; the leak checker is part
# of it.
export ASAN_OPTIONS="${ASAN_OPTIONS:-detect_leaks=1:abort_on_error=1}"
# libFuzzer's own driver allocates once and never frees; only that report is
# suppressed.
export LSAN_OPTIONS="${LSAN_OPTIONS:-suppressions=${PROJECT_ROOT}/crates/omt-fuzz/lsan.supp}"
TARGET_DIR="${PROJECT_ROOT}/target/sanitized"

usage() {
    echo "Usage: $0 test [cargo test args...] | build [cargo build args...] | fuzz [target...]" >&2
    exit 2
}

[[ $# -ge 1 ]] || usage
mode="$1"
shift

# Without -p, a test run covers the workspace; the fuzz harnesses build only
# in fuzz mode.
selection=(--workspace --exclude omt-fuzz)
for arg in "$@"; do
    if [[ "${arg}" == -p || "${arg}" == --package ]]; then selection=(); fi
done

case "${mode}" in
    test | build)
        RUSTFLAGS="${BASE_FLAGS}" cargo "+${NIGHTLY}" "${mode}" -Zbuild-std --offline \
            --target "${TARGET}" --target-dir "${TARGET_DIR}" --profile sanitize \
            "${selection[@]}" "$@"
        ;;
    fuzz)
        # Every crate is instrumented for coverage, not just the harnesses,
        # or libFuzzer would steer blind.
        FUZZ_FLAGS="${BASE_FLAGS} -Cpasses=sancov-module \
            -Cllvm-args=-sanitizer-coverage-level=4 \
            -Cllvm-args=-sanitizer-coverage-inline-8bit-counters \
            -Cllvm-args=-sanitizer-coverage-pc-table \
            -Cllvm-args=-sanitizer-coverage-trace-compares \
            -Clink-arg=${CLANG_RT}/libclang_rt.fuzzer-x86_64.a -Clink-arg=-lstdc++"
        RUSTFLAGS="${FUZZ_FLAGS}" cargo "+${NIGHTLY}" build -Zbuild-std --offline \
            --target "${TARGET}" --target-dir "${TARGET_DIR}" --profile sanitize \
            -p omt-fuzz --features fuzz --bins
        bin_dir="${TARGET_DIR}/${TARGET}/sanitize"
        targets=("$@")
        if [[ ${#targets[@]} -eq 0 ]]; then
            mapfile -t targets < <(find crates/omt-fuzz/src/bin -name 'fuzz_*.rs' -exec basename {} .rs \; | sort)
        fi
        failed=0
        for target in "${targets[@]}"; do
            name="${target#fuzz_}"
            work="${bin_dir}/fuzz-work/${name}"
            mkdir -p "${work}"
            echo "== ${target} (${FUZZ_SECONDS:-30}s)"
            if "${bin_dir}/${target}" -max_total_time="${FUZZ_SECONDS:-30}" -rss_limit_mb=2048 \
                -max_len=1048576 -artifact_prefix="${work}/" "${work}" "crates/omt-fuzz/corpus/${name}" \
                >"${work}.log" 2>&1; then
                echo "== ${target} passed"
            else
                cat "${work}.log"
                echo "== ${target} FAILED"
                failed=1
            fi
        done
        exit "${failed}"
        ;;
    *) usage ;;
esac
