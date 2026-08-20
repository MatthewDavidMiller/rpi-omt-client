#!/bin/bash
# Build and test the Rust deployer core, CLI, and terminal application.
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:-}"
if [[ $# -gt 1 || ( -n "${MODE}" && "${MODE}" != "--publish" ) ]]; then echo "Usage: $0 [--publish]" >&2; exit 2; fi
cd "${PROJECT_ROOT}"
command -v cargo >/dev/null 2>&1 || { echo "ERROR: cargo is required. Run: make install" >&2; exit 1; }
# The deployer embeds the appliance image, so the image is a build input rather
# than a separate artifact shipped beside it. Said here as well as in the build
# script so the ordering is a one-line failure instead of a cargo build error.
[[ -f "${PROJECT_ROOT}/omt-client-arm64.tar.gz" ]] || {
    echo "ERROR: omt-client-arm64.tar.gz is missing. The deployer embeds it. Run: make build-arm64" >&2
    exit 1
}
VERSION="${RPI_OMT_CLIENT_VERSION:-$("${PROJECT_ROOT}/scripts/detect-version.sh" "${PROJECT_ROOT}")}"

# The Linux deployer is the CLI and the terminal application. The egui
# application is a Windows artifact now and is built by
# scripts/build-windows-deployer.sh: on Linux it would reach the screen through
# libEGL, libGL, libX11, and libwayland-client, which it opens with dlopen at
# runtime. Those are the operator's graphics driver, linked against that
# machine's glibc, so a GUI build cannot be the one-binary-runs-anywhere
# artifact this ships.
TARGET=x86_64-unknown-linux-musl
rustup target list --installed 2>/dev/null | grep -Fxq "${TARGET}" || {
    echo "ERROR: Rust target ${TARGET} is required. Run: make install" >&2
    exit 1
}

RPI_OMT_CLIENT_VERSION="${VERSION}" cargo test --locked \
    -p omt-deployer-core -p rpi-omt-deploy -p rpi-omt-deploy-tui

# .cargo/config.toml turns crt-static *off* for both musl targets, because the
# receiver links Alpine's alsa-lib and there is no libasound.a to link against.
# The deployer binds no ALSA, and static linking is the whole point of shipping
# it as musl, so it is turned back on here.
#
# RUSTFLAGS replaces the config's per-target rustflags rather than extending
# them, so -Dwarnings is repeated deliberately -- without it the workspace's
# warning gate is silently lost for this build only.
STATIC_RUSTFLAGS="-Dwarnings -Ctarget-feature=+crt-static"

RUSTFLAGS="${STATIC_RUSTFLAGS}" RPI_OMT_CLIENT_VERSION="${VERSION}" \
    cargo build --locked --release --target "${TARGET}" \
    -p rpi-omt-deploy -p rpi-omt-deploy-tui

RELEASE_DIR="$(cargo metadata --format-version 1 --no-deps 2>/dev/null |
    sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')/${TARGET}/release"

# The portability claim is a property of the ELF headers, so it is read back
# out of them rather than assumed from the flags meant to produce it.
"${PROJECT_ROOT}/scripts/verify-linux-deployer.sh" "${RELEASE_DIR}/rpi-omt-deploy"
"${PROJECT_ROOT}/scripts/verify-linux-deployer.sh" "${RELEASE_DIR}/rpi-omt-deploy-tui"

# The CLI's own contract, against the binary that was just built.
tests/native/test_deployer_cli.sh "${RELEASE_DIR}/rpi-omt-deploy" "${PROJECT_ROOT}"

if [[ "${MODE}" == "--publish" ]]; then
    STAGE="${PROJECT_ROOT}/.build/deployer-publish.stage"
    PUBLISH="${PROJECT_ROOT}/.build/deployer-publish"
    rm -rf "${STAGE}"
    install -Dm755 "${RELEASE_DIR}/rpi-omt-deploy" "${STAGE}/bin/rpi-omt-deploy"
    install -Dm755 "${RELEASE_DIR}/rpi-omt-deploy-tui" "${STAGE}/bin/rpi-omt-deploy-tui"
    install -Dm644 LICENSE "${STAGE}/LICENSE"
    install -Dm644 THIRD_PARTY_NOTICES.txt "${STAGE}/THIRD_PARTY_NOTICES.txt"
    python3 scripts/generate-deployer-sbom.py --cargo-lock Cargo.lock --output "${STAGE}/deployer-sbom.cdx.json" --version "${VERSION}"
    rm -rf "${PUBLISH}"
    mv "${STAGE}" "${PUBLISH}"
    echo "Published Rust deployer package: ${PUBLISH}"
fi
