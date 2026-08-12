#!/bin/bash
# Build the amd64 image and verify the native OMT runtime contract.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
# shellcheck source=scripts/docker-test-env.sh
source "${PROJECT_ROOT}/scripts/docker-test-env.sh"

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'
IMAGE_TAG="omt-client:test-build"
ARM64_ARTIFACT_TAG="omt-client:test-build-arm64-artifacts"
ARM64_ARTIFACT_CONTAINER="omt-client-arm64-artifact-check"
MAX_RUNTIME_IMAGE_BYTES=$((128 * 1024 * 1024))
MAX_ARM64_ARTIFACT_IMAGE_BYTES=$((64 * 1024 * 1024))

cleanup() {
    if [[ -n "${CONTAINER_ENGINE:-}" ]]; then
        "${CONTAINER_ENGINE}" rm -f "${ARM64_ARTIFACT_CONTAINER}" >/dev/null 2>&1 || true
        "${CONTAINER_ENGINE}" rmi "${ARM64_ARTIFACT_TAG}" >/dev/null 2>&1 || true
        # Pre-commit sets SECURITY_SCAN_REUSE_IMAGE=1 so the following Trivy
        # image scan can reuse this amd64 tag instead of rebuilding it.
        if [[ "${SECURITY_SCAN_REUSE_IMAGE:-0}" != "1" ]]; then
            "${CONTAINER_ENGINE}" rmi "${IMAGE_TAG}" >/dev/null 2>&1 || true
        fi
    fi
}
trap cleanup EXIT

pass() { printf "${GREEN}PASS${NC}: %s\n" "$1"; }
fail() {
    printf "${RED}FAIL${NC}: %s\n" "$1" >&2
    exit 1
}

echo "Native OMT Container Build Integration Test"
echo "==========================================="

command -v python3 >/dev/null 2>&1 || fail "python3 is required"
# shellcheck disable=SC2310
ensure_test_container_engine || fail "Docker or Podman is required"
if [[ "${CONTAINER_ENGINE_KIND}" == "docker" ]] &&
   ! "${CONTAINER_ENGINE}" buildx version >/dev/null 2>&1; then
    fail "Docker buildx is required for the ARM64 builder-stage check"
fi

cd "${PROJECT_ROOT}"
# shellcheck disable=SC2310
container_engine_build \
    -f deploy/Dockerfile \
    --build-arg RPI_OMT_CLIENT_VERSION=vtest \
    -t "${IMAGE_TAG}" . || fail "amd64 container image build failed"
pass "amd64 container image built"

version_label="$("${CONTAINER_ENGINE}" inspect \
    --format '{{ index .Config.Labels "org.opencontainers.image.version" }}' \
    "${IMAGE_TAG}")"
license_label="$("${CONTAINER_ENGINE}" inspect \
    --format '{{ index .Config.Labels "org.opencontainers.image.licenses" }}' \
    "${IMAGE_TAG}")"
image_user="$("${CONTAINER_ENGINE}" inspect --format '{{ .Config.User }}' "${IMAGE_TAG}")"
[[ "${version_label}" == "vtest" ]] || fail "version label is missing"
[[ "${license_label}" == "MIT" ]] || fail "license label is wrong"
[[ "${image_user}" == "omt" ]] || fail "runtime user is not omt"
pass "image metadata identifies the version, MIT license, and non-root user"

runtime_image_size="$("${CONTAINER_ENGINE}" image inspect \
    --format '{{ .Size }}' "${IMAGE_TAG}")"
if [[ "${runtime_image_size}" =~ ^[0-9]+$ ]] &&
   (( runtime_image_size <= MAX_RUNTIME_IMAGE_BYTES )); then
    pass "runtime image remains at or below 128 MiB (${runtime_image_size} bytes)"
else
    fail "runtime image exceeds the 128 MiB size budget (${runtime_image_size} bytes)"
fi

saved_image="$(mktemp)"
if [[ "${CONTAINER_ENGINE_KIND}" == "podman" ]]; then
    save_command=("${CONTAINER_ENGINE}" save --format docker-archive -o "${saved_image}" "${IMAGE_TAG}")
else
    save_command=("${CONTAINER_ENGINE}" save -o "${saved_image}" "${IMAGE_TAG}")
fi
"${save_command[@]}" || fail "unable to save image for layer inspection"
if python3 - "${saved_image}" <<'PY'
import json
import sys
import tarfile

forbidden_prefixes = (
    "opt/omt-sdk",
    "src/native",
    "src/third_party/omt",
)
hits = []
with tarfile.open(sys.argv[1]) as archive:
    manifest = json.load(archive.extractfile("manifest.json"))
    for layer_name in manifest[0]["Layers"]:
        with tarfile.open(fileobj=archive.extractfile(layer_name), mode="r|*") as layer:
            for member in layer:
                name = member.name.lstrip("./")
                if any(name == prefix or name.startswith(prefix + "/")
                       for prefix in forbidden_prefixes):
                    hits.append(f"{layer_name}:{name}")
if hits:
    print("\n".join(hits[:20]), file=sys.stderr)
    raise SystemExit(1)
PY
then
    pass "final layers omit receiver source and full upstream source trees"
else
    fail "final image contains builder-only source content"
fi
rm -f "${saved_image}"

checks=(
    "test -x /usr/local/bin/omt-receiver"
    "test -x /usr/local/bin/omt-web"
    "test -x /usr/local/bin/entrypoint.sh && test -x /usr/local/bin/control-omt.sh && test -x /usr/local/bin/start-omt.sh"
    "! test -e /opt/venv && ! command -v python3 >/dev/null 2>&1"
    "grep -q '/usr/local/bin/omt-web' /app/runtime-sha256.manifest"
    "test \"\$(cat /app/RPI_OMT_CLIENT_VERSION)\" = vtest"
    "test -s /app/legal/LICENSE && test -s /app/legal/THIRD_PARTY_NOTICES.txt"
    "grep -Fq 'Copyright (c) 2026 Matthew David Miller' /app/legal/LICENSE && grep -Fq 'MIT License' /app/legal/LICENSE"
    "/usr/local/bin/omt-receiver --version | grep -Fxq vtest"
    "/usr/local/bin/omt-web --version | grep -Fxq vtest"
    "test \"\$(/usr/local/bin/omt-receiver discover --wait-ms 0 --json)\" = '[]'"
    "! find /usr/lib -name 'libstdc++.so*' -print -quit | grep -q ."
    "grep -Fq '\"bomFormat\": \"CycloneDX\"' /app/legal/runtime-sbom.cdx.json && grep -Fq '\"version\": \"vtest\"' /app/legal/runtime-sbom.cdx.json && grep -Fq '\"name\": \"axum\"' /app/legal/runtime-sbom.cdx.json && grep -Fq '\"name\": \"serde\"' /app/legal/runtime-sbom.cdx.json"
    "test -s /app/runtime-sha256.manifest && sha256sum --check /app/runtime-sha256.manifest >/dev/null"
    "! command -v gst-launch-1.0 >/dev/null 2>&1"
    "test \"\$HOME\" = /etc/omt && test \"\$(id -un)\" = omt"
)
labels=(
    "Rust OMT receiver is executable"
    "Rust Web frontend is executable"
    "OMT runtime scripts are executable"
    "Python and its virtual environment are absent"
    "integrity manifest covers the Rust Web frontend"
    "version file matches the build"
    "project license and third-party notices are packaged"
    "project copyright is exact"
    "Rust receiver reports the build version"
    "Rust Web frontend reports the build version"
    "zero-source discovery returns JSON"
    "runtime image contains no C++ standard library"
    "runtime CycloneDX SBOM identifies Rust components"
    "runtime SHA-256 manifest verifies"
    "retired GStreamer runtime is absent"
    "runtime identity and HOME are fixed"
)

for index in "${!checks[@]}"; do
    if timeout 30 "${CONTAINER_ENGINE}" run --rm \
        --entrypoint /bin/sh "${IMAGE_TAG}" -c "${checks[${index}]}"; then
        pass "${labels[${index}]}"
    else
        fail "${labels[${index}]}"
    fi
done

# The appliance only ever runs ARM64, so its builder stage is not an optional
# extra: without registered emulation this host cannot certify what it ships.
# `make install` registers it; `make setup-arm64-emulation` repairs it.
arm_probe_image="docker.io/library/debian:bookworm-slim@sha256:4724b8cc51e33e398f0e2e15e18d5ec2851ff0c2280647e1310bc1642182655d"
if ! timeout 30 "${CONTAINER_ENGINE}" run --rm --platform linux/arm64 \
    --entrypoint /bin/true "${arm_probe_image}" >/dev/null 2>&1; then
    fail "ARM64 emulation is unavailable; run: make setup-arm64-emulation"
fi

if [[ "${CONTAINER_ENGINE_KIND}" == "docker" ]]; then
    arm_build=(
        "${CONTAINER_ENGINE}" buildx build
        --platform linux/arm64
        --target runtime-artifacts
        --load
        -f deploy/Dockerfile
        -t "${ARM64_ARTIFACT_TAG}" .
    )
else
    arm_build=(
        "${CONTAINER_ENGINE}" build
        --format docker
        --layers=false
        --platform linux/arm64
        --target runtime-artifacts
        -f deploy/Dockerfile
        -t "${ARM64_ARTIFACT_TAG}" .
    )
fi
"${arm_build[@]}" || fail "ARM64 receiver builder stage failed"

arm64_artifact_image_size="$("${CONTAINER_ENGINE}" image inspect \
    --format '{{ .Size }}' "${ARM64_ARTIFACT_TAG}")"
if [[ "${arm64_artifact_image_size}" =~ ^[0-9]+$ ]] &&
   (( arm64_artifact_image_size <= MAX_ARM64_ARTIFACT_IMAGE_BYTES )); then
    pass "ARM64 artifact image remains at or below 64 MiB (${arm64_artifact_image_size} bytes)"
else
    fail "ARM64 artifact image exceeds the 64 MiB size budget (${arm64_artifact_image_size} bytes)"
fi

"${CONTAINER_ENGINE}" create \
    --name "${ARM64_ARTIFACT_CONTAINER}" "${ARM64_ARTIFACT_TAG}" >/dev/null
receiver_artifact="$(mktemp)"
web_artifact="$(mktemp)"
if "${CONTAINER_ENGINE}" cp \
       "${ARM64_ARTIFACT_CONTAINER}:/omt-receiver" "${receiver_artifact}" &&
   "${CONTAINER_ENGINE}" cp \
       "${ARM64_ARTIFACT_CONTAINER}:/omt-web" "${web_artifact}" &&
   [[ -s "${receiver_artifact}" ]] && [[ -s "${web_artifact}" ]]; then
    pass "ARM64 builder produced the Rust receiver and web artifacts"
else
    fail "ARM64 builder artifacts are missing"
fi
if python3 - "${receiver_artifact}" "${web_artifact}" <<'PY'
import struct
import sys

for path in sys.argv[1:]:
    with open(path, "rb") as artifact:
        header = artifact.read(20)
    if len(header) != 20 or header[:4] != b"\x7fELF" or header[4] != 2:
        raise SystemExit(f"{path} is not an ELF64 artifact")
    byte_order = {1: "<", 2: ">"}.get(header[5])
    if byte_order is None or struct.unpack(f"{byte_order}H", header[18:20])[0] != 183:
        raise SystemExit(f"{path} is not an AArch64 artifact")
PY
then
    pass "ARM64 builder artifacts are AArch64 ELF64 files"
else
    fail "ARM64 builder artifacts have the wrong architecture"
fi
rm -f "${receiver_artifact}" "${web_artifact}"

echo "==========================================="
echo -e "${GREEN}All Rust OMT image build tests passed!${NC}"
