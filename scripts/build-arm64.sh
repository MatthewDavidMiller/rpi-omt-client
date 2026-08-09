#!/bin/bash
# Build to a unique same-directory file and publish only a verified archive.

set -euo pipefail

export LC_ALL=C
umask 022

PROJECT_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
# `make install` provisions Podman, not Docker, so this build has to accept
# whichever engine the workstation actually has -- the same detection the live
# container tests use rather than a second, stricter rule.
# shellcheck source=scripts/docker-test-env.sh
source "${PROJECT_ROOT}/scripts/docker-test-env.sh"
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

ensure_test_container_engine || exit 1
"${PROJECT_ROOT}/scripts/check-arm64-emulation.sh"
echo "Building ARM64 image..."
if [[ "${CONTAINER_ENGINE_KIND}" == "podman" ]]; then
    # Podman has no buildx `--output type=docker`, so the archive is exported in
    # a second step. `--format docker` keeps the Dockerfile SHELL contract that
    # OCI metadata would drop, matching container_engine_build.
    # Podman normalizes a bare `-t omt-client` to `localhost/omt-client:latest`
    # and writes that into the archive, so `docker load` on the Pi produces a
    # tag that neither `docker run omt-client` nor compose.yml's `image:
    # omt-client` resolves. Tagging with the Docker Hub library prefix instead
    # makes Docker normalize it straight back to a bare `omt-client:latest`.
    podman_reference="docker.io/library/${IMAGE_NAME}:latest"
    "${CONTAINER_ENGINE}" build --format docker --platform linux/arm64 \
        --file "${PROJECT_ROOT}/deploy/Dockerfile" \
        --build-arg "RPI_OMT_CLIENT_VERSION=${RPI_OMT_CLIENT_VERSION}" \
        --iidfile "${staged_iid}" \
        -t "${podman_reference}" "${PROJECT_ROOT}"
    "${CONTAINER_ENGINE}" save --format docker-archive \
        --output "${staged_artifact}" "${podman_reference}"
else
    "${CONTAINER_ENGINE}" buildx build --platform linux/arm64 \
        --file "${PROJECT_ROOT}/deploy/Dockerfile" \
        --build-arg "RPI_OMT_CLIENT_VERSION=${RPI_OMT_CLIENT_VERSION}" \
        --iidfile "${staged_iid}" \
        --output "type=docker,dest=${staged_artifact}" \
        -t "${IMAGE_NAME}" "${PROJECT_ROOT}"
fi

if [[ ! -f "${staged_artifact}" || -L "${staged_artifact}" || \
      ! -s "${staged_artifact}" ]]; then
    echo "ERROR: The container engine did not produce a non-empty regular ARM64 archive." >&2
    exit 1
fi
if ! tar -tf "${staged_artifact}" >/dev/null; then
    echo "ERROR: The container engine produced an invalid or incomplete ARM64 archive." >&2
    exit 1
fi

# Both engines emit an *uncompressed* tar, so the published .tar.gz was not
# gzip at all and every deploy pushed ~86 MiB over SSH to a Raspberry Pi where
# ~27 MiB would do. `docker load` sniffs the archive rather than trusting the
# name, so compressing here needs no change on the appliance. -n omits the
# name and timestamp from the gzip header, which keeps the artifact
# byte-reproducible across rebuilds of an identical image.
compressed_artifact="${staged_artifact}.gz"
rm -f -- "${compressed_artifact}"
if ! gzip -9 -n -c -- "${staged_artifact}" > "${compressed_artifact}"; then
    echo "ERROR: Unable to compress the ARM64 archive." >&2
    exit 1
fi
rm -f -- "${staged_artifact}"
staged_artifact="${compressed_artifact}"
if [[ ! -f "${staged_artifact}" || -L "${staged_artifact}" || ! -s "${staged_artifact}" ]] || \
   ! tar -tzf "${staged_artifact}" >/dev/null; then
    echo "ERROR: The compressed ARM64 archive is invalid or incomplete." >&2
    exit 1
fi

if [[ ! -f "${staged_iid}" || ! -s "${staged_iid}" ]]; then
    echo "ERROR: The container engine did not publish the ARM64 image identity." >&2
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
