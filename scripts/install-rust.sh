#!/bin/bash
# Install the repository-pinned Rust toolchains after checksum verification.
#
# Two toolchains, each pinned to a dated release rather than a channel name:
#
#   * stable builds every shipped binary, for the three targets the project
#     ships: the aarch64 musl appliance, the static x86_64 musl deployer, and
#     the mingw Windows deployer.
#   * nightly runs only the gates the stable compiler cannot: the suites under
#     AddressSanitizer and the libFuzzer targets (-Zsanitizer, -Zbuild-std).
#     Nothing it compiles is ever published.
#
# rustup-init itself is pinned by SHA-256; rustup then verifies every
# component it downloads against the checksums in the dated channel manifest.
# rust-toolchain.toml names the same stable release, and
# tests/unit/test_supply_chain.sh holds the two to one version.

set -euo pipefail

RUSTUP_VERSION="1.29.1"
RUSTUP_LINUX_X86_64_SHA256="331228566cc931f32cd684f9bebc5956a5e5d7394c8d5de595f196a658ab98e6"
RUSTUP_LINUX_ARM64_SHA256="1ddf36182ac5d1782dbeefcb9bea7f5f9412d88f4a0d5047b1564a223624e8c6"
RUST_STABLE="1.98.1"
RUST_NIGHTLY="nightly-2026-09-24"
RUST_TARGETS=(aarch64-unknown-linux-musl x86_64-unknown-linux-musl x86_64-pc-windows-gnu)

# --build-only TARGET: just the stable toolchain and one target, for the
# appliance's builder stage, which needs neither the lints nor the nightly.
build_only=""
if [[ "${1:-}" == "--build-only" ]]; then
    build_only="${2:?--build-only needs a target triple}"
fi

: "${RUSTUP_HOME:?set RUSTUP_HOME to where the toolchains should live}"
: "${CARGO_HOME:?set CARGO_HOME to where the rustup proxies should live}"

case "$(uname -s)/$(uname -m)" in
    Linux/x86_64)
        host="x86_64-unknown-linux-musl"
        expected_sha256="${RUSTUP_LINUX_X86_64_SHA256}"
        ;;
    Linux/aarch64|Linux/arm64)
        host="aarch64-unknown-linux-musl"
        expected_sha256="${RUSTUP_LINUX_ARM64_SHA256}"
        ;;
    *)
        echo "ERROR: pinned Rust installation supports Linux x86_64 and arm64 only." >&2
        exit 1
        ;;
esac

download_dir="$(mktemp -d)"
trap 'rm -rf "${download_dir}"' EXIT
installer="${download_dir}/rustup-init"

curl -fsSLo "${installer}" \
    "https://static.rust-lang.org/rustup/archive/${RUSTUP_VERSION}/${host}/rustup-init"
printf '%s  %s\n' "${expected_sha256}" "${installer}" | sha256sum -c -
chmod 0755 "${installer}"

if [[ -n "${build_only}" ]]; then
    "${installer}" -y --no-modify-path --profile minimal \
        --default-host "${host}" --default-toolchain "${RUST_STABLE}"
    "${CARGO_HOME}/bin/rustup" target add --toolchain "${RUST_STABLE}" "${build_only}"
    exit 0
fi
"${installer}" -y --no-modify-path --profile minimal \
    --default-host "${host}" --default-toolchain "${RUST_STABLE}" \
    --component clippy,rustfmt
"${CARGO_HOME}/bin/rustup" target add --toolchain "${RUST_STABLE}" "${RUST_TARGETS[@]}"
"${CARGO_HOME}/bin/rustup" toolchain install "${RUST_NIGHTLY}" --profile minimal \
    --component rust-src,llvm-tools,miri
