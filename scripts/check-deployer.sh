#!/bin/bash
# Build and test the native deployer. --publish also resolves the hash-locked
# SDL3, Dear ImGui, and libssh2 source archives and stages a runnable package.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
MODE="${1:-}"
if [[ $# -gt 1 || ( -n "${MODE}" && "${MODE}" != "--publish" && "${MODE}" != "--integration-only" ) ]]; then
    echo "Usage: $0 [--publish|--integration-only]" >&2
    exit 2
fi

for tool in clang clang++ cmake ninja; do
    command -v "${tool}" >/dev/null 2>&1 || {
        echo "ERROR: ${tool} is required. Run: make install" >&2
        exit 1
    }
done

VERSION="${RPI_OMT_CLIENT_VERSION:-$("${PROJECT_ROOT}/scripts/detect-version.sh" "${PROJECT_ROOT}")}"
if [[ "${MODE}" == "--publish" ]]; then
    BUILD_DIR="${PROJECT_ROOT}/.build/native-deployer-release"
    GUI=ON
    BUILD_TYPE=Release
else
    BUILD_DIR="${PROJECT_ROOT}/.build/native-deployer-tests"
    GUI=OFF
    BUILD_TYPE=Debug
fi

configure_args=(
    -S "${PROJECT_ROOT}"
    -B "${BUILD_DIR}"
    -G Ninja
    -DCMAKE_BUILD_TYPE="${BUILD_TYPE}"
    -DCMAKE_C_COMPILER=clang
    -DCMAKE_CXX_COMPILER=clang++
    -DOMT_CLIENT_VERSION="${VERSION}"
    -DOMT_BUILD_RECEIVER=OFF
    -DOMT_BUILD_DEPLOYER=ON
    -DOMT_DEPLOYER_GUI="${GUI}"
    -DOMT_BUILD_TESTS=ON
)
for dependency in SDL3 IMGUI LIBSSH2; do
    mirror_name="RPI_OMT_${dependency}_SOURCE_DIR"
    mirror_value="${!mirror_name:-}"
    if [[ -n "${mirror_value}" ]]; then
        [[ -d "${mirror_value}" ]] || {
            echo "ERROR: ${mirror_name} is not a directory: ${mirror_value}" >&2
            exit 1
        }
        configure_args+=("-DFETCHCONTENT_SOURCE_DIR_${dependency}=${mirror_value}")
    fi
done

cmake "${configure_args[@]}"
cmake --build "${BUILD_DIR}" --parallel 2
ctest --test-dir "${BUILD_DIR}" --output-on-failure

if [[ "${MODE}" == "--publish" ]]; then
    PUBLISH_DIR="${PROJECT_ROOT}/.build/deployer-publish"
    STAGE_DIR="${PROJECT_ROOT}/.build/deployer-publish.stage"
    cmake -E remove_directory "${STAGE_DIR}"
    install_args=(--install "${BUILD_DIR}" --prefix "${STAGE_DIR}" --component Deployer)
    case "${OSTYPE:-}" in
        msys*|cygwin*|win32*) ;;
        *) install_args+=(--strip) ;;
    esac
    cmake "${install_args[@]}"
    cp "${PROJECT_ROOT}/LICENSE" "${PROJECT_ROOT}/THIRD_PARTY_NOTICES.txt" "${STAGE_DIR}/"
    python3 "${PROJECT_ROOT}/scripts/generate-deployer-sbom.py" \
        --output "${STAGE_DIR}/deployer-sbom.cdx.json" \
        --version "${VERSION}"
    cmake -E remove_directory "${PUBLISH_DIR}"
    cmake -E rename "${STAGE_DIR}" "${PUBLISH_DIR}"
    echo "Published native deployer package: ${PUBLISH_DIR}"
fi
