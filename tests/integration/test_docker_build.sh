#!/bin/bash
# Build the amd64 image and verify the native OMT runtime contract.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
# shellcheck source=scripts/docker-test-env.sh
source "${PROJECT_ROOT}/scripts/docker-test-env.sh"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'
IMAGE_TAG="omt-client:test-build"
ARM64_BUILDER_TAG="omt-client:test-build-arm64-builder"
ARM64_BUILDER_CONTAINER="omt-client-arm64-builder-check"

cleanup() {
    if [[ -n "${CONTAINER_ENGINE:-}" ]]; then
        "${CONTAINER_ENGINE}" rm -f "${ARM64_BUILDER_CONTAINER}" >/dev/null 2>&1 || true
        "${CONTAINER_ENGINE}" rmi "${ARM64_BUILDER_TAG}" >/dev/null 2>&1 || true
        "${CONTAINER_ENGINE}" rmi "${IMAGE_TAG}" >/dev/null 2>&1 || true
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
    "src/receiver",
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
    "test -s /usr/local/lib/libvmx.so"
    "test -x /usr/local/bin/entrypoint.sh && test -x /usr/local/bin/control-omt.sh && test -x /usr/local/bin/start-omt.sh"
    "/opt/venv/bin/python -c 'import importlib.resources as r; files=r.files(\"omt_client\"); assert (files / \"factory.py\").is_file(); assert all((files / \"templates\" / n).is_file() for n in (\"about.html\", \"system.html\", \"reboot_confirm.html\", \"login.html\")); assert (files / \"static\" / \"style.css\").is_file()'"
    "/opt/venv/bin/python -c 'import importlib.metadata as m; assert m.version(\"rpi-omt-client\")'"
    "! test -e /app/omt_client"
    "grep -q 'site-packages/omt_client/factory.py' /app/runtime-sha256.manifest"
    "! grep -q 'rpi_omt_client' /app/legal/THIRD_PARTY_NOTICES.txt"
    "/opt/venv/bin/python -c 'import json; d=json.load(open(\"/app/legal/runtime-sbom.cdx.json\", encoding=\"utf-8\")); assert not [c for c in d[\"components\"] if \"rpi-omt-client\" in str(c[\"name\"]).lower()]'"
    "test \"\$(cat /app/RPI_OMT_CLIENT_VERSION)\" = vtest"
    "test -s /app/legal/LICENSE && test -s /app/legal/THIRD_PARTY_NOTICES.txt"
    "grep -Fq 'PYTHON PACKAGE LICENSE FILE: flask-3.1.3.dist-info/licenses/LICENSE.txt' /app/legal/THIRD_PARTY_NOTICES.txt"
    "grep -Fq 'Copyright (c) 2026 Matthew David Miller' /app/legal/LICENSE && grep -Fq 'MIT License' /app/legal/LICENSE"
    "/usr/local/bin/omt-receiver --version | grep -Fxq vtest"
    "result=\$(/usr/local/bin/omt-receiver discover --wait-ms 0 --json) && printf '%s' \"\${result}\" | /opt/venv/bin/python -c 'import json,sys; assert isinstance(json.load(sys.stdin), list)'"
    "/opt/venv/bin/python -c 'import flask, flask_limiter, flask_wtf, gunicorn'"
    "/opt/venv/bin/python -c 'import json; p=\"/app/legal/runtime-sbom.cdx.json\"; d=json.load(open(p, encoding=\"utf-8\")); assert d[\"bomFormat\"] == \"CycloneDX\"; assert d[\"specVersion\"] == \"1.6\"; assert d[\"metadata\"][\"component\"][\"version\"] == \"vtest\"; assert d[\"metadata\"][\"component\"][\"licenses\"] == [{\"license\": {\"id\": \"MIT\"}}]; names={x[\"name\"] for x in d[\"components\"]}; assert {\"libomtnet\", \"libvmx\", \"omtplayer-derived-playback\"} <= names'"
    "test -s /app/runtime-sha256.manifest && sha256sum --check /app/runtime-sha256.manifest >/dev/null"
    "! command -v gst-launch-1.0 >/dev/null 2>&1 && ! find / -name 'libgstndi*' -print -quit 2>/dev/null | grep -q ."
    "test \"\$HOME\" = /etc/omt && test \"\$(id -un)\" = omt"
)
labels=(
    "native OMT receiver is executable"
    "native libvmx codec library is present"
    "OMT runtime scripts are executable"
    "Web views and static assets ship as package data"
    "application wheel is installed with metadata"
    "no stale copied application tree remains"
    "integrity manifest covers the installed application"
    "first-party wheel is not filed as a third-party notice"
    "first-party wheel is not listed as a PyPI dependency"
    "version file matches the build"
    "project license and third-party notices are packaged"
    "installed Python package license files are appended"
    "project copyright is exact"
    "native receiver reports the build version"
    "zero-source discovery returns JSON"
    "Python Web dependencies import"
    "runtime CycloneDX SBOM identifies native OMT components"
    "runtime SHA-256 manifest verifies"
    "retired GStreamer/NDI runtime is absent"
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

arm_sdk="mcr.microsoft.com/dotnet/sdk:10.0-alpine3.23@sha256:d8ee39817ca03a3757288e83c37ed73cc969a286c603b827c7cbe33add1c2d1c"
if timeout 30 "${CONTAINER_ENGINE}" run --rm --platform linux/arm64 \
    --entrypoint /bin/true "${arm_sdk}" >/dev/null 2>&1; then
    if [[ "${CONTAINER_ENGINE_KIND}" == "docker" ]]; then
        arm_build=(
            "${CONTAINER_ENGINE}" buildx build
            --platform linux/arm64
            --target receiver-builder
            --load
            -f deploy/Dockerfile
            -t "${ARM64_BUILDER_TAG}" .
        )
    else
        arm_build=(
            "${CONTAINER_ENGINE}" build
            --format docker
            --platform linux/arm64
            --target receiver-builder
            -f deploy/Dockerfile
            -t "${ARM64_BUILDER_TAG}" .
        )
    fi
    "${arm_build[@]}" || fail "ARM64 receiver builder stage failed"
    "${CONTAINER_ENGINE}" create \
        --name "${ARM64_BUILDER_CONTAINER}" "${ARM64_BUILDER_TAG}" >/dev/null
    receiver_artifact="$(mktemp)"
    vmx_artifact="$(mktemp)"
    if "${CONTAINER_ENGINE}" cp \
           "${ARM64_BUILDER_CONTAINER}:/out/receiver/omt-receiver" "${receiver_artifact}" &&
       "${CONTAINER_ENGINE}" cp \
           "${ARM64_BUILDER_CONTAINER}:/out/native/libvmx.so" "${vmx_artifact}" &&
       [[ -s "${receiver_artifact}" && -s "${vmx_artifact}" ]]; then
        pass "ARM64 builder produced the receiver and libvmx"
    else
        fail "ARM64 builder artifacts are missing"
    fi
    rm -f "${receiver_artifact}" "${vmx_artifact}"
elif [[ "${REQUIRE_ARM64_BUILD:-0}" == "1" ]]; then
    fail "ARM64 emulation is required but unavailable"
else
    printf '%bSKIP%b: ARM64 builder check requires registered emulation\n' \
        "${YELLOW}" "${NC}"
fi

echo "==========================================="
echo -e "${GREEN}All native OMT image build tests passed!${NC}"
