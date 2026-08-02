#!/bin/bash
# Locked restore, analysis, tests, coverage, and optional Windows publication.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
DOTNET_ROOT="${PROJECT_ROOT}/.build/dotnet"
DOTNET="${DOTNET_ROOT}/dotnet"
SOLUTION="${PROJECT_ROOT}/src/deployer/RpiOmt.Deployer.slnx"
TEST_PROJECT="${PROJECT_ROOT}/src/deployer/RpiOmt.Deployer.Tests/RpiOmt.Deployer.Tests.csproj"
INTEGRATION_PROJECT="${PROJECT_ROOT}/src/deployer/RpiOmt.Deployer.IntegrationTests/RpiOmt.Deployer.IntegrationTests.csproj"
APP_PROJECT="${PROJECT_ROOT}/src/deployer/RpiOmt.Deployer.App/RpiOmt.Deployer.App.csproj"
PUBLISHED_EXE="${PROJECT_ROOT}/dist/rpi-omt-client-deployer-windows-x64.exe"
PUBLISHED_SBOM="${PROJECT_ROOT}/dist/rpi-omt-client-deployer-windows-x64.cdx.json"

PUBLISH=false
INTEGRATION=false
INTEGRATION_ONLY=false
while (($# > 0)); do
    case "$1" in
        --publish) PUBLISH=true ;;
        --integration) INTEGRATION=true ;;
        --integration-only) INTEGRATION_ONLY=true ;;
        -h|--help)
            echo "Usage: $0 [--publish] [--integration|--integration-only]"
            exit 0
            ;;
        *)
            echo "Usage: $0 [--publish] [--integration|--integration-only]" >&2
            exit 2
            ;;
    esac
    shift
done

[[ -x "${DOTNET}" ]] || {
    echo "ERROR: repository-local .NET SDK is missing. Run: make test-setup" >&2
    exit 1
}
[[ "$("${DOTNET}" --version)" == "10.0.302" ]] || {
    echo "ERROR: repository-local .NET SDK must be exactly 10.0.302." >&2
    exit 1
}

export DOTNET_ROOT
export DOTNET_CLI_HOME="${PROJECT_ROOT}/.build/dotnet-home"
export NUGET_PACKAGES="${PROJECT_ROOT}/.build/nuget-packages"
export DOTNET_NOLOGO=1
export DOTNET_SKIP_FIRST_TIME_EXPERIENCE=1
export PATH="${DOTNET_ROOT}:${PATH}"
export RPI_OMT_CLIENT_VERSION
RPI_OMT_CLIENT_VERSION="$("${SCRIPT_DIR}/detect-version.sh" "${PROJECT_ROOT}")"
mkdir -p "${DOTNET_CLI_HOME}" "${NUGET_PACKAGES}"

if [[ "${INTEGRATION_ONLY}" == "true" ]]; then
    "${DOTNET}" test "${INTEGRATION_PROJECT}" --configuration Release \
        --no-restore --no-build --logger "console;verbosity=normal"
    exit 0
fi

echo "=== Locked NuGet restore and vulnerability audit ==="
for project in \
    "${PROJECT_ROOT}/src/deployer/RpiOmt.Deployer.Core/RpiOmt.Deployer.Core.csproj" \
    "${APP_PROJECT}" \
    "${TEST_PROJECT}" \
    "${INTEGRATION_PROJECT}"; do
    "${DOTNET}" restore "${project}" --locked-mode --runtime win-x64 \
        -p:NuGetAudit=true -p:NuGetAuditMode=all -p:TreatWarningsAsErrors=true
done

echo "=== C# format and analyzer gate ==="
"${DOTNET}" format "${SOLUTION}" --verify-no-changes --no-restore --verbosity minimal
"${DOTNET}" build "${SOLUTION}" --configuration Release --no-restore \
    -p:TreatWarningsAsErrors=true -m:1

coverage_root="$(mktemp -d "${PROJECT_ROOT}/.build/deployer-coverage.XXXXXX")"
publish_root=""
artifact_temp=""
cleanup() {
    rm -rf "${coverage_root}"
    if [[ -n "${publish_root}" ]]; then
        rm -rf "${publish_root}"
    fi
    if [[ -n "${artifact_temp}" ]]; then
        rm -f "${artifact_temp}"
    fi
}
trap cleanup EXIT

echo "=== Core unit and Avalonia headless tests ==="
"${DOTNET}" test "${TEST_PROJECT}" --configuration Release --no-restore --no-build \
    --settings "${PROJECT_ROOT}/src/deployer/RpiOmt.Deployer.Tests/coverage.runsettings" \
    --results-directory "${coverage_root}" --collect "XPlat Code Coverage" \
    --logger "console;verbosity=normal"
coverage_file="$(find "${coverage_root}" -name coverage.cobertura.xml -type f -print -quit)"
[[ -n "${coverage_file}" ]] || {
    echo "ERROR: Coverlet did not produce coverage.cobertura.xml." >&2
    exit 1
}
python3 "${SCRIPT_DIR}/check-dotnet-coverage.py" "${coverage_file}" --minimum 95

if [[ "${INTEGRATION}" == "true" ]]; then
    echo "=== Alpine userland integration tests ==="
    "${DOTNET}" test "${INTEGRATION_PROJECT}" --configuration Release \
        --no-restore --no-build --logger "console;verbosity=normal"
fi

if [[ "${PUBLISH}" != "true" ]]; then
    exit 0
fi

echo "=== Self-contained Windows x64 publish ==="
publish_root="$(mktemp -d "${PROJECT_ROOT}/.build/windows-publish.XXXXXX")"
"${DOTNET}" publish "${APP_PROJECT}" --configuration Release --runtime win-x64 \
    --self-contained true --no-restore --output "${publish_root}" \
    -p:PublishSingleFile=true -p:IncludeNativeLibrariesForSelfExtract=true \
    -p:DebugType=None -p:DebugSymbols=false -p:TreatWarningsAsErrors=true
staged_exe="${publish_root}/RpiOmt.Deployer.App.exe"
[[ -s "${staged_exe}" ]] || {
    echo "ERROR: dotnet publish did not produce ${staged_exe}." >&2
    exit 1
}
python3 "${SCRIPT_DIR}/verify-windows-pe.py" "${staged_exe}"

mkdir -p "$(dirname "${PUBLISHED_EXE}")"
artifact_temp="$(dirname "${PUBLISHED_EXE}")/.rpi-omt-client-deployer-windows-x64.exe.$$"
install -m 0755 "${staged_exe}" "${artifact_temp}"
python3 "${SCRIPT_DIR}/verify-windows-pe.py" "${artifact_temp}"
mv -f "${artifact_temp}" "${PUBLISHED_EXE}"
artifact_temp=""
python3 "${SCRIPT_DIR}/generate-windows-sbom.py" \
    --lock-file "${PROJECT_ROOT}/src/deployer/RpiOmt.Deployer.App/packages.lock.json" \
    --output "${PUBLISHED_SBOM}" \
    --version "${RPI_OMT_CLIENT_VERSION}"
echo "Published atomically: ${PUBLISHED_EXE}"
echo "Published SBOM: ${PUBLISHED_SBOM}"
