#!/bin/bash
# Build and test the dependency-free receiver core with analyzers and coverage.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
DOTNET="${PROJECT_ROOT}/.build/dotnet/dotnet"
CORE="${PROJECT_ROOT}/src/receiver/RpiOmt.Receiver.Core/RpiOmt.Receiver.Core.csproj"
TESTS="${PROJECT_ROOT}/src/receiver/RpiOmt.Receiver.Core.Tests/RpiOmt.Receiver.Core.Tests.csproj"

if [[ ! -x "${DOTNET}" ]]; then
    echo "ERROR: repo-local .NET SDK is missing. Run: make test-setup" >&2
    exit 1
fi

export DOTNET_CLI_HOME="${PROJECT_ROOT}/.build/dotnet-home"
export NUGET_PACKAGES="${PROJECT_ROOT}/.build/nuget-packages"
export DOTNET_NOLOGO=1

"${DOTNET}" restore "${TESTS}" --locked-mode \
    -p:NuGetAudit=true -p:NuGetAuditMode=all -p:TreatWarningsAsErrors=true
"${DOTNET}" build "${CORE}" --no-restore -c Release \
    -p:TreatWarningsAsErrors=true
"${DOTNET}" test "${TESTS}" --no-restore -c Release \
    --settings "${PROJECT_ROOT}/src/receiver/RpiOmt.Receiver.Core.Tests/coverage.runsettings" \
    --collect:"XPlat Code Coverage" \
    --results-directory "${PROJECT_ROOT}/.build/receiver-test-results"

coverage_file="$(find "${PROJECT_ROOT}/.build/receiver-test-results" \
    -name coverage.cobertura.xml -type f -printf '%T@ %p\n' |
    sort -nr | head -n 1 | cut -d' ' -f2-)"
if [[ -z "${coverage_file}" ]]; then
    echo "ERROR: receiver-core coverage report was not produced" >&2
    exit 1
fi
"${PROJECT_ROOT}/tests/.venv/bin/python" \
    "${PROJECT_ROOT}/scripts/check-dotnet-coverage.py" \
    "${coverage_file}" --minimum 95
