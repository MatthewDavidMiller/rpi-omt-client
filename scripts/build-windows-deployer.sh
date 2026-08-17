#!/bin/bash
# Cross-compile the Rust CLI and egui deployer for Windows x86-64.
#
# --no-publish compiles and header-verifies the cross build without staging a
# package. The pre-commit gate uses it: a package is stamped with the version
# the commit carries, so publishing belongs to the post-commit hook.
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:-}"
if [[ $# -gt 1 || ( -n "${MODE}" && "${MODE}" != "--no-publish" ) ]]; then
    echo "Usage: $0 [--no-publish]" >&2
    exit 2
fi
cd "${PROJECT_ROOT}"
TARGET=x86_64-pc-windows-gnu
rustup target list --installed 2>/dev/null | grep -Fxq "${TARGET}" || { echo "ERROR: Rust target ${TARGET} is required. Run: make install" >&2; exit 1; }
# The .exe carries the appliance image inside it, so the image has to exist
# before the cross build starts.
[[ -f "${PROJECT_ROOT}/omt-client-arm64.tar.gz" ]] || {
    echo "ERROR: omt-client-arm64.tar.gz is missing. The deployer embeds it. Run: make build-arm64" >&2
    exit 1
}
VERSION="${RPI_OMT_CLIENT_VERSION:-$("${PROJECT_ROOT}/scripts/detect-version.sh" "${PROJECT_ROOT}")}"
RPI_OMT_CLIENT_VERSION="${VERSION}" cargo build --locked --release --target "${TARGET}" -p rpi-omt-deploy -p rpi-omt-deployer --features rpi-omt-deployer/desktop
if [[ "${MODE}" == "--no-publish" ]]; then
    "${PROJECT_ROOT}/scripts/verify-windows-deployer.sh" --console "target/${TARGET}/release/rpi-omt-deploy.exe"
    "${PROJECT_ROOT}/scripts/verify-windows-deployer.sh" "target/${TARGET}/release/rpi-omt-deployer.exe"
    echo "Verified Windows Rust deployer cross build; not published"
    exit 0
fi
STAGE="${PROJECT_ROOT}/.build/deployer-publish-windows.stage"
PUBLISH="${PROJECT_ROOT}/.build/deployer-publish-windows"
rm -rf "${STAGE}"
install -Dm755 "target/${TARGET}/release/rpi-omt-deploy.exe" "${STAGE}/bin/rpi-omt-deploy.exe"
install -Dm755 "target/${TARGET}/release/rpi-omt-deployer.exe" "${STAGE}/bin/rpi-omt-deployer.exe"
install -Dm644 LICENSE "${STAGE}/LICENSE"
install -Dm644 THIRD_PARTY_NOTICES.txt "${STAGE}/THIRD_PARTY_NOTICES.txt"
python3 scripts/generate-deployer-sbom.py --cargo-lock Cargo.lock --output "${STAGE}/deployer-sbom.cdx.json" --version "${VERSION}"
"${PROJECT_ROOT}/scripts/verify-windows-deployer.sh" --console "${STAGE}/bin/rpi-omt-deploy.exe"
"${PROJECT_ROOT}/scripts/verify-windows-deployer.sh" "${STAGE}/bin/rpi-omt-deployer.exe"
rm -rf "${PUBLISH}"
mv "${STAGE}" "${PUBLISH}"
echo "Published Windows Rust deployer package: ${PUBLISH}"
