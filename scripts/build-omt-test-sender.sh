#!/bin/bash
# Build and stage the first-party OMT test sender (crates/omt-test-sender).
#
#   --target auto     this workstation (the default)
#   --target aarch64  a Raspberry Pi 4 or 5, cross-compiled against the
#                     Alpine sysroot the appliance links
#
# The workstation build is a static PIE, so it runs on whatever distribution
# the workstation has. Rust has no static PIE for aarch64 musl, and a
# fixed-address static binary would give up ASLR; the Pi always runs Alpine,
# so its sender links Alpine's musl dynamically, as the appliance does.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BUILD_HOME="${OMT_TEST_SENDER_HOME:-${PROJECT_ROOT}/.build/omt-test-sender}"

usage() {
    echo "Usage: $0 [--target auto|aarch64]" >&2
}

requested_target="auto"
if [[ $# -eq 2 && "$1" == "--target" ]]; then
    requested_target="$2"
elif [[ $# -ne 0 ]]; then
    usage
    exit 2
fi
case "${requested_target}" in
    auto | aarch64) ;;
    *) usage; exit 2 ;;
esac

command -v cargo >/dev/null 2>&1 || { echo "ERROR: cargo is required. Run: make install" >&2; exit 1; }
host_target="$(cc -dumpmachine)"
cd "${PROJECT_ROOT}"

if [[ "${requested_target}" == "auto" ]]; then
    rust_target=x86_64-unknown-linux-musl
    target="${host_target}"
    export RUSTFLAGS="-C target-feature=+crt-static -C relocation-model=pie"
else
    rust_target=aarch64-unknown-linux-musl
    target="aarch64-alpine-linux-musl"
    # .cargo/config.toml's flags for this target: dynamic musl from the
    # sysroot, linked by the appliance's clang/lld wrapper.
    export OMT_SYSROOT="${OMT_SYSROOT:-/opt/sysroot-aarch64}"
fi
echo "Building the OMT test sender for ${target}..."
cargo build --release --offline --locked --target "${rust_target}" \
    --target-dir "${PROJECT_ROOT}/target/sender" -p omt-test-sender
source_binary="${PROJECT_ROOT}/target/sender/${rust_target}/release/omt-test-sender"
[[ -x "${source_binary}" ]] || {
    echo "ERROR: sender build did not produce ${source_binary}" >&2
    exit 1
}

artifact_dir="${BUILD_HOME}/artifacts/${target}"
mkdir -p "${artifact_dir}/bin"
install -m 0755 "${source_binary}" "${artifact_dir}/bin/omt-test-sender"
printf '%s\n' "target=${target}" "source=crates/omt-test-sender" > "${artifact_dir}/BUILD-INFO"

if [[ "${target}" == "${host_target}" ]]; then
    current_tmp="${BUILD_HOME}/.current.$$"
    ln -s "artifacts/${target}" "${current_tmp}"
    mv -Tf "${current_tmp}" "${BUILD_HOME}/current"
    echo "Built and activated: ${artifact_dir}"
else
    echo "Built cross-target artifact (not activated on ${host_target}): ${artifact_dir}"
fi
