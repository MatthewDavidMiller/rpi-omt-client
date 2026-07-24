#!/bin/bash
# Structural checks for reproducible local builds and commit-time security gates.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
PRE_COMMIT_HOOK="${PROJECT_ROOT}/.githooks/pre-commit"
CHECK_DEPLOYER="${PROJECT_ROOT}/scripts/check-deployer.sh"
SDK_INSTALLER="${PROJECT_ROOT}/scripts/install-dotnet-sdk.sh"
PE_VERIFIER="${PROJECT_ROOT}/scripts/verify-windows-pe.py"
PYTHON_AUDIT="${PROJECT_ROOT}/scripts/audit-python-deps.sh"

PASS=0
FAIL=0
pass() { printf 'PASS: %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf 'FAIL: %s\n' "$1" >&2; FAIL=$((FAIL + 1)); }

assert_literal() {
    local file="$1" text="$2" label="$3"
    if grep -Fq -- "${text}" "${file}"; then pass "${label}"; else fail "${label}"; fi
}

assert_contains() {
    local file="$1" pattern="$2" label="$3"
    if grep -Eq -- "${pattern}" "${file}"; then pass "${label}"; else fail "${label}"; fi
}

assert_absent() {
    local path="$1" label="$2"
    if [[ ! -e "${path}" ]]; then pass "${label}"; else fail "${label}"; fi
}

assert_executable() {
    local path="$1" label="$2"
    if [[ -x "${path}" ]]; then pass "${label}"; else fail "${label}"; fi
}

assert_ignored() {
    local path="$1" label="$2"
    if git -c core.excludesFile=/dev/null -C "${PROJECT_ROOT}" \
            check-ignore --no-index -q -- "${path}"; then
        pass "${label}"
    else
        fail "${label}"
    fi
}

assert_not_ignored() {
    local path="$1" label="$2"
    if git -c core.excludesFile=/dev/null -C "${PROJECT_ROOT}" \
            check-ignore --no-index -q -- "${path}"; then
        fail "${label}"
    else
        pass "${label}"
    fi
}

echo "=== Supply Chain Guardrail Tests ==="

assert_absent "${PROJECT_ROOT}/.github" "GitHub automation is absent"
assert_absent "${PROJECT_ROOT}/deploy_gui" "Legacy Tkinter deployer is absent"
assert_absent "${PROJECT_ROOT}/.githooks/pre-push" "Pre-push hook is absent"
assert_absent "${PROJECT_ROOT}/scripts/requirements-audit.txt" "CI-only Python audit lock is absent"

assert_executable "${PRE_COMMIT_HOOK}" "Pre-commit hook is executable"
assert_executable "${CHECK_DEPLOYER}" "Shared deployer gate is executable"
assert_executable "${SDK_INSTALLER}" "Pinned SDK installer is executable"
assert_executable "${PE_VERIFIER}" "PE verifier is executable"
assert_executable "${PROJECT_ROOT}/scripts/check-dotnet-coverage.py" "Coverage verifier is executable"
assert_executable "${PROJECT_ROOT}/scripts/check-legal-notices.py" "Legal notice verifier is executable"
assert_executable "${PROJECT_ROOT}/scripts/generate-runtime-sbom.py" "Runtime SBOM generator is executable"

assert_ignored ".env.local" "Local environment variants are ignored"
assert_not_ignored ".env.example" "Environment examples remain visible"
assert_ignored "vars.yml" "Sensitive installer variables are ignored"
assert_ignored "omt-client-arm64.tar.gz" "Generated ARM64 image archive is ignored"
assert_not_ignored "ndi-client-arm64.tar.gz" "Legacy NDI archives remain visible"
assert_ignored ".mypy_cache/probe" "Mypy cache is ignored"
assert_ignored ".ruff_cache/probe" "Ruff cache is ignored"
assert_ignored ".venv/pyvenv.cfg" "Developer virtual environments are ignored"
assert_ignored ".vs/settings.json" "Visual Studio state is ignored"
assert_ignored "src/deployer/project.csproj.user" "Visual Studio user files are ignored"
assert_ignored ".claude/settings.local.json" "Local Claude permissions are ignored"
assert_not_ignored "source-snapshot.tar.gz" "Unrelated root archives remain visible"
assert_not_ignored "third_party/source-snapshot.tar.gz" "Nested source archives remain visible"
for win32_source in DnsApi.cs OMTDiscoveryWin32.cs Win32Platform.cs; do
    if [[ -s "${PROJECT_ROOT}/third_party/omt/libomtnet/src/win32/${win32_source}" ]]; then
        pass "libomtnet Win32 source is present: ${win32_source}"
    else
        fail "libomtnet Win32 source is present: ${win32_source}"
    fi
    assert_not_ignored \
        "third_party/omt/libomtnet/src/win32/${win32_source}" \
        "libomtnet Win32 source is visible: ${win32_source}"
done

assert_literal "${PROJECT_ROOT}/global.json" '"version": "10.0.302"' ".NET SDK version is exact"
assert_literal "${PROJECT_ROOT}/global.json" '"rollForward": "disable"' ".NET SDK roll-forward is disabled"
assert_literal "${SDK_INSTALLER}" 'SDK_VERSION="10.0.302"' "SDK installer matches global.json"
assert_contains "${SDK_INSTALLER}" '^SDK_SHA512="[0-9a-f]{128}"$' "SDK archive has a pinned SHA-512"
assert_literal "${SDK_INSTALLER}" 'sha512sum -c -' "SDK archive checksum is enforced"

packages="${PROJECT_ROOT}/Directory.Packages.props"
for package_version in \
    'Avalonia" Version="12.1.0' \
    'Avalonia.Headless" Version="12.1.0' \
    'SSH.NET" Version="2025.1.0' \
    'xunit.v3" Version="3.2.2' \
    'Microsoft.NET.Test.Sdk" Version="18.8.1' \
    'coverlet.collector" Version="10.0.1'; do
    assert_literal "${packages}" "${package_version}" "Pinned NuGet package: ${package_version%%\"*}"
done

for project in App Core Tests IntegrationTests; do
    lock_file="${PROJECT_ROOT}/src/deployer/RpiOmt.Deployer.${project}/packages.lock.json"
    if [[ -s "${lock_file}" ]]; then pass "${project} NuGet lock is committed"; else fail "${project} NuGet lock is committed"; fi
done

assert_literal "${PROJECT_ROOT}/Directory.Build.props" '<NuGetAudit>true</NuGetAudit>' "NuGet audit is enabled"
assert_literal "${PROJECT_ROOT}/Directory.Build.props" '<NuGetAuditMode>all</NuGetAuditMode>' "NuGet audits transitive dependencies"
assert_literal "${PROJECT_ROOT}/Directory.Build.props" '<TreatWarningsAsErrors>true</TreatWarningsAsErrors>' "C# warnings fail the gate"
assert_literal "${CHECK_DEPLOYER}" '--locked-mode' "Shared gate performs locked restore"
assert_literal "${CHECK_DEPLOYER}" '-p:NuGetAudit=true' "Shared gate explicitly enforces NuGet audit"
assert_literal "${CHECK_DEPLOYER}" 'format "${SOLUTION}" --verify-no-changes' "Shared gate verifies C# formatting"
assert_literal "${CHECK_DEPLOYER}" '--minimum 95' "Core branch coverage threshold is 95 percent"
assert_literal "${CHECK_DEPLOYER}" '--runtime win-x64' "Publish targets Windows x64"
assert_literal "${CHECK_DEPLOYER}" '--self-contained true' "Publish is self-contained"
assert_literal "${CHECK_DEPLOYER}" '-p:PublishSingleFile=true' "Publish produces a single file"
assert_literal "${CHECK_DEPLOYER}" 'verify-windows-pe.py' "Published artifact is inspected as PE"
assert_literal "${CHECK_DEPLOYER}" 'mv -f "${artifact_temp}" "${PUBLISHED_EXE}"' "Verified artifact replaces the stable path atomically"

assert_contains "${PROJECT_ROOT}/Makefile" '^test-deployer:' "Make exposes test-deployer"
assert_contains "${PROJECT_ROOT}/Makefile" '^build-windows-deployer:' "Make exposes build-windows-deployer"
assert_literal "${PROJECT_ROOT}/Makefile" './scripts/setup-hooks.sh' "make install configures the trusted hook path"
assert_literal "${PRE_COMMIT_HOOK}" './scripts/test-local.sh --full' "Pre-commit runs the full local suite"
assert_literal "${PRE_COMMIT_HOOK}" './scripts/audit-python-deps.sh' "Pre-commit runs Python dependency audits"
assert_literal "${PRE_COMMIT_HOOK}" './scripts/security-scan.sh' "Pre-commit runs Trivy scans"
assert_literal "${PROJECT_ROOT}/scripts/test-local.sh" '"${PROJECT_ROOT}/scripts/check-deployer.sh" --publish' "Default and full suites publish Windows deployer"
assert_literal "${PROJECT_ROOT}/scripts/test-local.sh" '"${PROJECT_ROOT}/scripts/check-deployer.sh" --integration-only' "Container tier runs C# Pi-userland tests"

assert_literal "${PYTHON_AUDIT}" 'run_audit audit_hash_locked "requirements/runtime.txt"' "Python audit covers application dependencies"
assert_literal "${PYTHON_AUDIT}" 'run_audit audit_pinned_no_deps "tests/requirements-dev.txt"' "Python audit covers development dependencies"
if [[ "$(grep -c '^run_audit ' "${PYTHON_AUDIT}")" -eq 2 ]]; then
    pass "Python audit has exactly two retained lock scopes"
else
    fail "Python audit has exactly two retained lock scopes"
fi

assert_literal "${PROJECT_ROOT}/deploy/Dockerfile" 'dotnet publish src/receiver/RpiOmt.Receiver/RpiOmt.Receiver.csproj' "Container builds the pinned OMT receiver"
assert_literal "${PROJECT_ROOT}/src/receiver/RpiOmt.Receiver/packages.lock.json" '"libomtnet"' "Receiver NuGet graph is locked"
assert_literal "${PROJECT_ROOT}/deploy/Dockerfile" '--require-hashes' "Container Python install is hash locked"
assert_literal "${PROJECT_ROOT}/deploy/Dockerfile" 'runtime-sbom.cdx.json' "Container emits a runtime SBOM"
assert_literal "${PROJECT_ROOT}/scripts/security-scan.sh" '--skip-files vars.yml' "Security scan does not inspect sensitive vars.yml"
assert_literal "${PROJECT_ROOT}/src/deployer/RpiOmt.Deployer.Core/ImageBuildService.cs" 'tonistiigi/binfmt@sha256:400a4873b838d1b89194d982c45e5fb3cda4593fbfd7e08a02e76b03b21166f0' "Deployer pins the privileged binfmt image"

audit_test_dir="$(mktemp -d)"
trap 'rm -rf "${audit_test_dir}"' EXIT
cat > "${audit_test_dir}/pip-audit" <<'EOF'
#!/bin/bash
[[ "${1:-}" == "--version" ]] && exit 0
printf '%s\n' "$*" >> "${FAKE_AUDIT_LOG}"
exit 1
EOF
chmod +x "${audit_test_dir}/pip-audit"
if FAKE_AUDIT_LOG="${audit_test_dir}/calls" \
   PIP_AUDIT_BIN="${audit_test_dir}/pip-audit" \
   PIP_AUDIT_CACHE_DIR="${audit_test_dir}/cache" \
   "${PYTHON_AUDIT}" > "${audit_test_dir}/output" 2>&1; then
    fail "Dependency audit propagates findings"
elif [[ "$(wc -l < "${audit_test_dir}/calls")" -eq 2 ]] && \
     grep -q 'failed for 2 lock file(s)' "${audit_test_dir}/output"; then
    pass "Dependency audit checks both locks before failing"
else
    fail "Dependency audit checks both locks before failing"
fi

echo
echo "Results: ${PASS} passed, ${FAIL} failed"
[[ "${FAIL}" -eq 0 ]]
