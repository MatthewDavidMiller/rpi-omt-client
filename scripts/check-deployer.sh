#!/bin/bash
# Build and test the C deployer: core, SSH client, CLI, and terminal
# application. --publish also stages the Linux package.
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:-}"
if [[ $# -gt 1 || ( -n "${MODE}" && "${MODE}" != "--publish" ) ]]; then echo "Usage: $0 [--publish]" >&2; exit 2; fi
cd "${PROJECT_ROOT}"
# The deployer embeds the appliance image, so the image is a build input rather
# than a separate artifact shipped beside it. Said here as well as in the
# capsule generator so the ordering is a one-line failure.
[[ -f "${PROJECT_ROOT}/omt-client-arm64.tar.gz" ]] || {
    echo "ERROR: omt-client-arm64.tar.gz is missing. The deployer embeds it. Run: make build-arm64" >&2
    exit 1
}
for tool in gcc clang make python3 sshd ssh-keygen; do
    command -v "${tool}" >/dev/null 2>&1 || { echo "ERROR: ${tool} is required. Run: make install" >&2; exit 1; }
done
VERSION="${RPI_OMT_CLIENT_VERSION:-$("${PROJECT_ROOT}/scripts/detect-version.sh" "${PROJECT_ROOT}")}"
JOBS="$(nproc)"
mk() { make -s -f mk/c.mk OMT_VERSION="${VERSION}" -j"${JOBS}" "$@"; }

# Every deployer suite under ASan and UBSan: the core, the SSH client's parts,
# the terminal application, and the real capsule.
mk BUILD=asan test-deploy test-deploy-capsule
ASAN_OUT="$(mk BUILD=asan print-out)"
mk BUILD=asan "${ASAN_OUT}/tests/ssh_interop" "${ASAN_OUT}/bin/rpi-omt-deploy"
# The SSH client against a real OpenSSH server, still sanitized.
tests/integration/test_ssh_client.sh "${ASAN_OUT}/tests/ssh_interop" "${ASAN_OUT}/bin/rpi-omt-deploy"

# The shipping build: a static PIE against musl, so one binary runs on every
# distribution. The portability claim is a property of the ELF headers, so it
# is read back out of them rather than assumed from the flags.
mk CC=gcc BUILD=release LDFLAGS=-static-pie deployer
RELEASE_DIR="$(mk CC=gcc BUILD=release print-out)/bin"
"${PROJECT_ROOT}/scripts/verify-linux-deployer.sh" "${RELEASE_DIR}/rpi-omt-deploy"
"${PROJECT_ROOT}/scripts/verify-linux-deployer.sh" "${RELEASE_DIR}/rpi-omt-deploy-tui"
# The CLI's own contract, against the binary that ships.
tests/native/test_deployer_cli.sh "${RELEASE_DIR}/rpi-omt-deploy" "${PROJECT_ROOT}"

if [[ "${MODE}" == "--publish" ]]; then
    STAGE="${PROJECT_ROOT}/.build/deployer-publish.stage"
    PUBLISH="${PROJECT_ROOT}/.build/deployer-publish"
    rm -rf "${STAGE}"
    install -Dm755 "${RELEASE_DIR}/rpi-omt-deploy" "${STAGE}/bin/rpi-omt-deploy"
    install -Dm755 "${RELEASE_DIR}/rpi-omt-deploy-tui" "${STAGE}/bin/rpi-omt-deploy-tui"
    install -Dm644 LICENSE "${STAGE}/LICENSE"
    install -Dm644 THIRD_PARTY_NOTICES.txt "${STAGE}/THIRD_PARTY_NOTICES.txt"
    OPENSSL_VERSION="$(openssl version | awk '{print $2}')"
    MUSL_VERSION="$(apk info -v musl 2>/dev/null | sed -n 's/^musl-\([0-9.]*\).*/\1/p' | head -1)"
    python3 scripts/generate-deployer-sbom.py --output "${STAGE}/deployer-sbom.cdx.json" \
        --version "${VERSION}" --openssl-version "${OPENSSL_VERSION}" \
        --musl-version "${MUSL_VERSION:-unknown}"
    rm -rf "${PUBLISH}"
    mv "${STAGE}" "${PUBLISH}"
    echo "Published deployer package: ${PUBLISH}"
fi
