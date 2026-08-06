#!/bin/bash
# Cross-compile, verify, and stage the Windows x86-64 deployment application
# from a Linux host using the mingw-w64 toolchain.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
if [[ $# -gt 0 ]]; then
    echo "Usage: $0" >&2
    exit 2
fi

MINGW_TRIPLE="x86_64-w64-mingw32"
for tool in cmake ninja "${MINGW_TRIPLE}-gcc" "${MINGW_TRIPLE}-objdump"; do
    command -v "${tool}" >/dev/null 2>&1 || {
        echo "ERROR: ${tool} is required for the Windows cross build. Run: make install" >&2
        exit 1
    }
done

VERSION="${RPI_OMT_CLIENT_VERSION:-$("${PROJECT_ROOT}/scripts/detect-version.sh" "${PROJECT_ROOT}")}"
BUILD_DIR="${PROJECT_ROOT}/.build/deployer-windows"
PUBLISH_DIR="${PROJECT_ROOT}/.build/deployer-publish-windows"
STAGE_DIR="${PROJECT_ROOT}/.build/deployer-publish-windows.stage"
EXECUTABLE="${BUILD_DIR}/src/native/deployer/rpi-omt-deployer.exe"

configure_args=(
    -S "${PROJECT_ROOT}"
    -B "${BUILD_DIR}"
    -G Ninja
    -DCMAKE_TOOLCHAIN_FILE="${PROJECT_ROOT}/cmake/toolchains/windows-x86_64-mingw.cmake"
    -DCMAKE_BUILD_TYPE=Release
    -DOMT_CLIENT_VERSION="${VERSION}"
    -DOMT_BUILD_RECEIVER=OFF
    -DOMT_BUILD_DEPLOYER=ON
    -DOMT_DEPLOYER_GUI=ON
    -DOMT_BUILD_TESTS=ON
)
# Offline or restricted builders may point the locked dependencies at trusted
# source trees they have already verified, exactly as the host build does.
for dependency in SDL3 NUKLEAR LIBSSH2; do
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

# The cross build cannot execute its own output, so the artifact contract is
# checked directly: a 64-bit Windows GUI binary with ASLR, DEP, and 64-bit
# address-space randomization, importing nothing but system DLLs.
[[ -s "${EXECUTABLE}" ]] || {
    echo "ERROR: the Windows cross build produced no executable" >&2
    exit 1
}
"${PROJECT_ROOT}/scripts/verify-windows-deployer.sh" "${EXECUTABLE}"

cmake -E remove_directory "${STAGE_DIR}"
cmake --install "${BUILD_DIR}" --prefix "${STAGE_DIR}" --component Deployer --strip
cp "${PROJECT_ROOT}/LICENSE" "${PROJECT_ROOT}/THIRD_PARTY_NOTICES.txt" "${STAGE_DIR}/"

# The compiler runtime is linked into the .exe rather than supplied by the
# operating system, so the inventory has to name the toolchain that produced it.
gcc_version="$("${MINGW_TRIPLE}-gcc" -dumpversion)"
mingw_runtime_version="$(
    printf '#include <_mingw_mac.h>\n__MINGW64_VERSION_STR\n' |
        "${MINGW_TRIPLE}-gcc" -E -P -xc - | tail -n 1 | tr -d '" '
)"
[[ -n "${gcc_version}" && -n "${mingw_runtime_version}" ]] || {
    echo "ERROR: unable to identify the cross toolchain runtime versions" >&2
    exit 1
}
python3 "${PROJECT_ROOT}/scripts/generate-deployer-sbom.py" \
    --output "${STAGE_DIR}/deployer-sbom.cdx.json" \
    --version "${VERSION}" \
    --mingw-gcc-version "${gcc_version}" \
    --mingw-runtime-version "${mingw_runtime_version}"
cmake -E remove_directory "${PUBLISH_DIR}"
cmake -E rename "${STAGE_DIR}" "${PUBLISH_DIR}"
echo "Published Windows deployer package: ${PUBLISH_DIR}"
