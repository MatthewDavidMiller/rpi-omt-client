#!/bin/bash
# Cross-compile the Windows x86-64 deployer -- the CLI and the terminal
# application -- with mingw-w64 against the pinned OpenSSL.
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
TARGET=x86_64-w64-mingw32
command -v "${TARGET}-gcc" >/dev/null 2>&1 || { echo "ERROR: ${TARGET}-gcc is required. Run: make install" >&2; exit 1; }
# The .exe carries the appliance image inside it, so the image has to exist
# before the cross build starts.
[[ -f "${PROJECT_ROOT}/omt-client-arm64.tar.gz" ]] || {
    echo "ERROR: omt-client-arm64.tar.gz is missing. The deployer embeds it. Run: make build-arm64" >&2
    exit 1
}
VERSION="${RPI_OMT_CLIENT_VERSION:-$("${PROJECT_ROOT}/scripts/detect-version.sh" "${PROJECT_ROOT}")}"
OPENSSL_PREFIX="$("${PROJECT_ROOT}/scripts/build-openssl.sh" "${TARGET}")"
mk() {
    make -s -f mk/c.mk CC="${TARGET}-gcc" BUILD=release OMT_VERSION="${VERSION}" \
        OPENSSL_PREFIX="${OPENSSL_PREFIX}" -j"$(nproc)" "$@"
}
mk deployer
RELEASE_DIR="$(mk print-out)/bin"
if [[ "${MODE}" == "--no-publish" ]]; then
    "${PROJECT_ROOT}/scripts/verify-windows-deployer.sh" --console "${RELEASE_DIR}/rpi-omt-deploy.exe"
    "${PROJECT_ROOT}/scripts/verify-windows-deployer.sh" --console "${RELEASE_DIR}/rpi-omt-deploy-tui.exe"
    echo "Verified Windows deployer cross build; not published"
    exit 0
fi
STAGE="${PROJECT_ROOT}/.build/deployer-publish-windows.stage"
PUBLISH="${PROJECT_ROOT}/.build/deployer-publish-windows"
rm -rf "${STAGE}"
install -Dm755 "${RELEASE_DIR}/rpi-omt-deploy.exe" "${STAGE}/bin/rpi-omt-deploy.exe"
install -Dm755 "${RELEASE_DIR}/rpi-omt-deploy-tui.exe" "${STAGE}/bin/rpi-omt-deploy-tui.exe"
install -Dm644 LICENSE "${STAGE}/LICENSE"
install -Dm644 THIRD_PARTY_NOTICES.txt "${STAGE}/THIRD_PARTY_NOTICES.txt"
OPENSSL_VERSION="$(sed -n 's/^OPENSSL_VERSION="\(.*\)"$/\1/p' scripts/build-openssl.sh)"
GCC_VERSION="$("${TARGET}-gcc" -dumpversion)"
MINGW_VERSION="$(sed -n 's/^#define __MINGW64_VERSION_STR *"\(.*\)"$/\1/p' \
    "/usr/${TARGET}/include/_mingw_mac.h" 2>/dev/null | head -1)"
python3 scripts/generate-deployer-sbom.py --output "${STAGE}/deployer-sbom.cdx.json" \
    --version "${VERSION}" --openssl-version "${OPENSSL_VERSION}" \
    --mingw-gcc-version "${GCC_VERSION}" --mingw-runtime-version "${MINGW_VERSION:-unknown}"
"${PROJECT_ROOT}/scripts/verify-windows-deployer.sh" --console "${STAGE}/bin/rpi-omt-deploy.exe"
"${PROJECT_ROOT}/scripts/verify-windows-deployer.sh" --console "${STAGE}/bin/rpi-omt-deploy-tui.exe"
rm -rf "${PUBLISH}"
mv "${STAGE}" "${PUBLISH}"
echo "Published Windows deployer package: ${PUBLISH}"
