#!/bin/bash
# Build and test the dependency-free receiver core with analyzers and coverage.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
DOTNET="${PROJECT_ROOT}/.build/dotnet/dotnet"
CORE="${PROJECT_ROOT}/src/receiver/RpiOmt.Receiver.Core/RpiOmt.Receiver.Core.csproj"
RECEIVER="${PROJECT_ROOT}/src/receiver/RpiOmt.Receiver/RpiOmt.Receiver.csproj"
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
"${DOTNET}" restore "${RECEIVER}" --locked-mode \
    -p:NuGetAudit=true -p:NuGetAuditMode=all -p:TreatWarningsAsErrors=true
"${DOTNET}" build "${CORE}" --no-restore -c Release \
    -p:TreatWarningsAsErrors=true
"${DOTNET}" build "${RECEIVER}" --no-restore -c Release \
    -p:TreatWarningsAsErrors=true
"${DOTNET}" test "${TESTS}" --no-restore -c Release \
    --settings "${PROJECT_ROOT}/src/receiver/RpiOmt.Receiver.Core.Tests/coverage.runsettings" \
    --collect:"XPlat Code Coverage" \
    --results-directory "${PROJECT_ROOT}/.build/receiver-test-results"

# Newest report wins. `sed -n 1s//p` rather than `head -n 1`: under
# `pipefail`, a `head` that exits after one line leaves `sort` writing into a
# closed pipe, and that SIGPIPE becomes the pipeline's status and fails this
# gate. Whether the race is lost depends on how much output `sort` still has
# left, so the run directories accumulating here decide it -- the gate passed
# on a clean tree and failed once enough of them had piled up. `sed` reads its
# input to the end and cannot lose that race.
coverage_file="$(find "${PROJECT_ROOT}/.build/receiver-test-results" \
    -name coverage.cobertura.xml -type f -printf '%T@ %p\n' |
    sort -nr | sed -n '1s/^[^ ]* //p')"
if [[ -z "${coverage_file}" ]]; then
    echo "ERROR: receiver-core coverage report was not produced" >&2
    exit 1
fi
"${PROJECT_ROOT}/tests/.venv/bin/python" \
    "${PROJECT_ROOT}/scripts/check-dotnet-coverage.py" \
    "${coverage_file}" --minimum 95
