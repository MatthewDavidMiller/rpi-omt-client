#!/bin/bash
# Build and test the Rust deployer core, CLI, and egui application.
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:-}"
if [[ $# -gt 1 || ( -n "${MODE}" && "${MODE}" != "--publish" ) ]]; then echo "Usage: $0 [--publish]" >&2; exit 2; fi
cd "${PROJECT_ROOT}"
command -v cargo >/dev/null 2>&1 || { echo "ERROR: cargo is required. Run: make install" >&2; exit 1; }
VERSION="${RPI_OMT_CLIENT_VERSION:-$("${PROJECT_ROOT}/scripts/detect-version.sh" "${PROJECT_ROOT}")}"
# The desktop feature is on for the test build too: without it the egui
# application is not compiled at all, so `cargo test -p rpi-omt-deployer` would
# report a pass over a crate it never built.
RPI_OMT_CLIENT_VERSION="${VERSION}" cargo test --locked -p omt-deployer-core -p rpi-omt-deploy -p rpi-omt-deployer --features rpi-omt-deployer/desktop
RPI_OMT_CLIENT_VERSION="${VERSION}" cargo build --locked --release -p rpi-omt-deploy -p rpi-omt-deployer --features rpi-omt-deployer/desktop
# The CLI's own contract, against the binary that was just built.
tests/native/test_deployer_cli.sh "${PROJECT_ROOT}/target/release/rpi-omt-deploy" "${PROJECT_ROOT}"
if [[ "${MODE}" == "--publish" ]]; then
    STAGE="${PROJECT_ROOT}/.build/deployer-publish.stage"
    PUBLISH="${PROJECT_ROOT}/.build/deployer-publish"
    rm -rf "${STAGE}"
    install -Dm755 target/release/rpi-omt-deploy "${STAGE}/bin/rpi-omt-deploy"
    install -Dm755 target/release/rpi-omt-deployer "${STAGE}/bin/rpi-omt-deployer"
    install -Dm644 LICENSE "${STAGE}/LICENSE"
    install -Dm644 THIRD_PARTY_NOTICES.txt "${STAGE}/THIRD_PARTY_NOTICES.txt"
    python3 scripts/generate-deployer-sbom.py --cargo-lock Cargo.lock --output "${STAGE}/deployer-sbom.cdx.json" --version "${VERSION}"
    rm -rf "${PUBLISH}"
    mv "${STAGE}" "${PUBLISH}"
    echo "Published Rust deployer package: ${PUBLISH}"
fi
