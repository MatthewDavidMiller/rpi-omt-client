#!/bin/bash
# Build to a unique same-directory file and publish only a verified archive.

set -euo pipefail

export LC_ALL=C
umask 022

PROJECT_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
IMAGE_NAME="${IMAGE_NAME:-omt-client}"
ARM64_TARBALL="${ARM64_TARBALL:-${PROJECT_ROOT}/omt-client-arm64.tar.gz}"
BUILD_METADATA_DIR="${BUILD_METADATA_DIR:-${PROJECT_ROOT}/.build}"
RPI_OMT_CLIENT_VERSION="${RPI_OMT_CLIENT_VERSION:-$("${PROJECT_ROOT}/scripts/detect-version.sh" "${PROJECT_ROOT}")}"

artifact_directory="$(dirname -- "${ARM64_TARBALL}")"
artifact_name="$(basename -- "${ARM64_TARBALL}")"
mkdir -p "${BUILD_METADATA_DIR}" "${artifact_directory}"
if [[ -L "${BUILD_METADATA_DIR}" || -L "${artifact_directory}" || \
      ! -d "${BUILD_METADATA_DIR}" || ! -d "${artifact_directory}" ]]; then
    echo "ERROR: ARM64 build output directories must be real directories." >&2
    exit 1
fi
staged_artifact="$(mktemp "${artifact_directory}/.${artifact_name}.build.XXXXXX.tmp")"
staged_iid="$(mktemp "${BUILD_METADATA_DIR}/.arm64.iid.XXXXXX.tmp")"
rm -f -- "${staged_artifact}"

cleanup() {
    rm -f -- "${staged_artifact}" "${staged_iid}"
}
trap cleanup EXIT

"${PROJECT_ROOT}/scripts/check-arm64-emulation.sh"
echo "Building ARM64 image..."
docker buildx build --platform linux/arm64 \
    --file "${PROJECT_ROOT}/deploy/Dockerfile" \
    --build-arg "RPI_OMT_CLIENT_VERSION=${RPI_OMT_CLIENT_VERSION}" \
    --iidfile "${staged_iid}" \
    --output "type=docker,dest=${staged_artifact}" \
    -t "${IMAGE_NAME}" "${PROJECT_ROOT}"

if [[ ! -f "${staged_artifact}" || -L "${staged_artifact}" || \
      ! -s "${staged_artifact}" ]]; then
    echo "ERROR: Docker did not produce a non-empty regular ARM64 archive." >&2
    exit 1
fi
if ! tar -tf "${staged_artifact}" >/dev/null; then
    echo "ERROR: Docker produced an invalid or incomplete ARM64 archive." >&2
    exit 1
fi
if [[ ! -f "${staged_iid}" || ! -s "${staged_iid}" ]]; then
    echo "ERROR: Docker did not publish the ARM64 image identity." >&2
    exit 1
fi

sync -f "${staged_artifact}"
sync -f "${staged_iid}"
mv -fT -- "${staged_artifact}" "${ARM64_TARBALL}"
mv -fT -- "${staged_iid}" "${BUILD_METADATA_DIR}/arm64.iid"
sync -d "${artifact_directory}"
sync -d "${BUILD_METADATA_DIR}"

echo "Image digest: $(cat "${BUILD_METADATA_DIR}/arm64.iid")"
echo "Built: ${ARM64_TARBALL}"
