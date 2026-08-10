#!/bin/bash
# Build and stage the first-party Rust OMT test sender. The ARM64 musl target
# produces the same self-contained executable format used by the receiver.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BUILD_HOME="${OMT_TEST_SENDER_HOME:-${PROJECT_ROOT}/.build/omt-test-sender}"

usage() {
    echo "Usage: $0 [--target auto|HOST_TRIPLE|aarch64-unknown-linux-musl]" >&2
}

requested_target="auto"
if [[ $# -eq 2 && "$1" == "--target" ]]; then
    requested_target="$2"
elif [[ $# -ne 0 ]]; then
    usage
    exit 2
fi

for tool in cargo rustc; do
    command -v "${tool}" >/dev/null 2>&1 || {
        echo "ERROR: ${tool} is required. Run: make install" >&2
        exit 1
    }
done

host_target="$(rustc -vV | awk '/^host:/ { print $2 }')"
[[ -n "${host_target}" ]] || {
    echo "ERROR: unable to determine the Rust host target" >&2
    exit 1
}
[[ "${requested_target}" == "auto" ]] && requested_target="${host_target}"
[[ "${requested_target}" =~ ^[A-Za-z0-9_.-]+$ ]] || {
    echo "ERROR: invalid Rust target triple" >&2
    exit 2
}

target_args=(--target "${requested_target}")
target_environment=()
if [[ "${requested_target}" == "aarch64-unknown-linux-musl" ]]; then
    command -v rustup >/dev/null 2>&1 || {
        echo "ERROR: rustup is required to verify the ARM64 Rust target" >&2
        exit 1
    }
    rustup target list --installed | grep -Fxq "${requested_target}" || {
        echo "ERROR: ${requested_target} is not installed. Run: make install" >&2
        exit 1
    }
    target_environment=(
        CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld
        RUSTFLAGS=-Clink-self-contained=yes
    )
fi

echo "Building Rust OMT test sender for ${requested_target}..."
(
    cd "${PROJECT_ROOT}"
    env "${target_environment[@]}" cargo build --locked --release \
        -p omt-test-sender "${target_args[@]}"
)

source_binary="${PROJECT_ROOT}/target/${requested_target}/release/omt-test-sender"
[[ -x "${source_binary}" ]] || {
    echo "ERROR: sender build did not produce ${source_binary}" >&2
    exit 1
}

artifact_dir="${BUILD_HOME}/artifacts/${requested_target}"
mkdir -p "${artifact_dir}/bin"
install -m 0755 "${source_binary}" "${artifact_dir}/bin/omt-test-sender"
printf '%s\n' "target=${requested_target}" "source=crates/omt-test-sender" \
    > "${artifact_dir}/BUILD-INFO"

if [[ "${requested_target}" == "${host_target}" ]]; then
    current_tmp="${BUILD_HOME}/.current.$$"
    ln -s "artifacts/${requested_target}" "${current_tmp}"
    mv -Tf "${current_tmp}" "${BUILD_HOME}/current"
    echo "Built and activated: ${artifact_dir}"
else
    echo "Built cross-target artifact (not activated on ${host_target}): ${artifact_dir}"
fi
