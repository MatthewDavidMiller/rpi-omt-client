#!/bin/bash
# Build and test the deployer: core, SSH client, CLI, and browser
# frontend. --publish also stages the Linux package.
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:-}"
if [[ $# -gt 1 || ( -n "${MODE}" && "${MODE}" != "--publish" ) ]]; then echo "Usage: $0 [--publish]" >&2; exit 2; fi
cd "${PROJECT_ROOT}"
# The deployer embeds the appliance image, so the image is a build input rather
# than a separate artifact shipped beside it. Said here as well as in the
# capsule build script so the ordering is a one-line failure.
[[ -f "${PROJECT_ROOT}/omt-client-arm64.tar.gz" ]] || {
    echo "ERROR: omt-client-arm64.tar.gz is missing. The deployer embeds it. Run: make build-arm64" >&2
    exit 1
}
for tool in cargo clang python3 sshd ssh-keygen; do
    command -v "${tool}" >/dev/null 2>&1 || { echo "ERROR: ${tool} is required. Run: make install" >&2; exit 1; }
done
VERSION="${RPI_OMT_CLIENT_VERSION:-$("${PROJECT_ROOT}/scripts/detect-version.sh" "${PROJECT_ROOT}")}"
export OMT_VERSION="${VERSION}"
# Every build here embeds the real capsule; a missing member is a build error
# rather than an empty capsule.
export OMT_CAPSULE_REQUIRED=1
TARGET=x86_64-unknown-linux-musl
DEPLOYER_CRATES=(-p omt-deploy -p rpi-omt-deploy -p rpi-omt-deploy-gui)

# Every deployer suite under AddressSanitizer: the core, the SSH client's
# parts, the browser frontend, and the real capsule.
scripts/rust-sanitized.sh test "${DEPLOYER_CRATES[@]}"
scripts/rust-sanitized.sh build -p omt-deploy --bin ssh_interop -p rpi-omt-deploy --bin rpi-omt-deploy
SANITIZED_DIR="${PROJECT_ROOT}/target/sanitized/${TARGET}/sanitize"
# The SSH client against a real OpenSSH server, still sanitized.
tests/integration/test_ssh_client.sh "${SANITIZED_DIR}/ssh_interop" "${SANITIZED_DIR}/rpi-omt-deploy"

# The shipping build: a static PIE against musl and a static OpenSSL, so one
# binary runs on every distribution. The portability claim is a property of
# the ELF headers, so it is read back out of them rather than assumed from the
# flags. Its own target directory keeps these flags from invalidating the
# workspace's dynamically linked build.
#
# It links this toolbox's Alpine musl and GCC's static unwinder through the
# appliance's linker wrapper, rather than the copies rustc bundles, so the
# package's SBOM can name the musl it carries from the apk database.
OMT_SYSROOT=/ CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="${PROJECT_ROOT}/scripts/cargo-linker-x86_64.sh" \
    OMT_OPENSSL_STATIC=1 OMT_OPENSSL_DIR=/usr \
    RUSTFLAGS="-C target-feature=+crt-static -C relocation-model=pie -C link-self-contained=no" \
    cargo build --release --offline --locked --target "${TARGET}" \
    --target-dir "${PROJECT_ROOT}/target/deployer" -p rpi-omt-deploy -p rpi-omt-deploy-gui
RELEASE_DIR="${PROJECT_ROOT}/target/deployer/${TARGET}/release"
"${PROJECT_ROOT}/scripts/verify-linux-deployer.sh" "${RELEASE_DIR}/rpi-omt-deploy"
"${PROJECT_ROOT}/scripts/verify-linux-deployer.sh" "${RELEASE_DIR}/rpi-omt-deploy-gui"
# The CLI's own contract, against the binary that ships.
tests/native/test_deployer_cli.sh "${RELEASE_DIR}/rpi-omt-deploy" "${PROJECT_ROOT}"

if [[ "${MODE}" == "--publish" ]]; then
    STAGE="${PROJECT_ROOT}/.build/deployer-publish.stage"
    PUBLISH="${PROJECT_ROOT}/.build/deployer-publish"
    rm -rf "${STAGE}"
    install -Dm755 "${RELEASE_DIR}/rpi-omt-deploy" "${STAGE}/bin/rpi-omt-deploy"
    install -Dm755 "${RELEASE_DIR}/rpi-omt-deploy-gui" "${STAGE}/bin/rpi-omt-deploy-gui"
    install -Dm644 LICENSE "${STAGE}/LICENSE"
    install -Dm644 THIRD_PARTY_NOTICES.txt "${STAGE}/THIRD_PARTY_NOTICES.txt"
    OPENSSL_VERSION="$(openssl version | awk '{print $2}')"
    MUSL_VERSION="$(apk info -v musl 2>/dev/null | sed -n 's/^musl-\([0-9.]*\).*/\1/p' | head -1)"
    python3 scripts/generate-deployer-sbom.py --output "${STAGE}/deployer-sbom.cdx.json" \
        --version "${VERSION}" --openssl-version "${OPENSSL_VERSION}" \
        --rust-version "$(rustc --version | awk '{print $2}')" \
        --gcc-version "$(gcc -dumpversion)" --musl-version "${MUSL_VERSION:-unknown}"
    rm -rf "${PUBLISH}"
    mv "${STAGE}" "${PUBLISH}"
    echo "Published deployer package: ${PUBLISH}"
fi
