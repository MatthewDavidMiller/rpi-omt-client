#!/bin/bash
# Install the exact repository-local .NET SDK after verifying Microsoft's SHA-512.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
SDK_VERSION="10.0.302"
SDK_URL="https://builds.dotnet.microsoft.com/dotnet/Sdk/${SDK_VERSION}/dotnet-sdk-${SDK_VERSION}-linux-x64.tar.gz"
SDK_SHA512="10069bec8783596484a610332f090d562802a41b9b40e3327a5a5688b572e10c296ae300f940d40461f23c157ed1b0843c2f8e6b3f20d8d8d9d83432d8143bac"
DOTNET_INSTALL_ROOT="${PROJECT_ROOT}/.build/dotnet"

if [[ "$(uname -s)" != "Linux" || "$(uname -m)" != "x86_64" ]]; then
    echo "ERROR: the pinned deployer SDK bootstrap supports Linux x86-64 only." >&2
    exit 1
fi

if [[ -x "${DOTNET_INSTALL_ROOT}/dotnet" ]] && \
   [[ "$("${DOTNET_INSTALL_ROOT}/dotnet" --version 2>/dev/null)" == "${SDK_VERSION}" ]]; then
    echo ".NET SDK ${SDK_VERSION} is already installed in ${DOTNET_INSTALL_ROOT}."
    exit 0
fi

for tool in curl sha512sum tar; do
    command -v "${tool}" >/dev/null 2>&1 || {
        echo "ERROR: ${tool} is required to install the local .NET SDK." >&2
        exit 1
    }
done

mkdir -p "${PROJECT_ROOT}/.build"
download_path="$(mktemp "${TMPDIR:-/tmp}/rpi-omt-dotnet.XXXXXX.tar.gz")"
stage_path="$(mktemp -d "${PROJECT_ROOT}/.build/dotnet-stage.XXXXXX")"
trap 'rm -f "${download_path}"; rm -rf "${stage_path}"' EXIT

echo "Downloading .NET SDK ${SDK_VERSION}..."
curl --fail --location --proto '=https' --tlsv1.2 --output "${download_path}" "${SDK_URL}"
printf '%s  %s\n' "${SDK_SHA512}" "${download_path}" | sha512sum -c -
tar -xzf "${download_path}" -C "${stage_path}"
[[ "$("${stage_path}/dotnet" --version)" == "${SDK_VERSION}" ]] || {
    echo "ERROR: extracted SDK version does not match ${SDK_VERSION}." >&2
    exit 1
}

previous_path="${PROJECT_ROOT}/.build/dotnet.previous.$$"
if [[ -e "${DOTNET_INSTALL_ROOT}" ]]; then
    mv "${DOTNET_INSTALL_ROOT}" "${previous_path}"
fi
if mv "${stage_path}" "${DOTNET_INSTALL_ROOT}"; then
    if [[ -e "${previous_path}" ]]; then
        rm -rf "${previous_path}"
    fi
else
    if [[ -e "${previous_path}" ]]; then
        mv "${previous_path}" "${DOTNET_INSTALL_ROOT}"
    fi
    exit 1
fi

echo "Installed .NET SDK ${SDK_VERSION} in ${DOTNET_INSTALL_ROOT}."
