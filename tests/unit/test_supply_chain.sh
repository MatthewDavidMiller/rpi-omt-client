#!/bin/bash
# Structural checks for reproducible native builds and commit-time security gates.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
PRE_COMMIT_HOOK="${PROJECT_ROOT}/.githooks/pre-commit"
CHECK_DEPLOYER="${PROJECT_ROOT}/scripts/check-deployer.sh"
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
assert_absent "${PROJECT_ROOT}/src/receiver" "C# receiver tree is absent"
assert_absent "${PROJECT_ROOT}/src/deployer" "C# deployer tree is absent"
assert_absent "${PROJECT_ROOT}/third_party/omt/libomtnet" "C# OMT transport snapshot is absent"
assert_absent "${PROJECT_ROOT}/third_party/omt/omtplayer" "C# playback snapshot is absent"
assert_absent "${PROJECT_ROOT}/global.json" ".NET SDK lock is absent"
if find "${PROJECT_ROOT}" -path "${PROJECT_ROOT}/.build" -prune -o \
       -path "${PROJECT_ROOT}/.git" -prune -o \
       \( -name '*.cs' -o -name '*.csproj' -o -name '*.sln' -o -name '*.slnx' -o -name '*.axaml' \) \
       -print -quit | grep -q .; then
    fail "Repository contains no C#/.NET project source"
else
    pass "Repository contains no C#/.NET project source"
fi

assert_executable "${PRE_COMMIT_HOOK}" "Pre-commit hook is executable"
assert_executable "${CHECK_DEPLOYER}" "Native deployer gate is executable"
assert_executable "${PROJECT_ROOT}/tools/test-receiver.sh" "Native receiver gate is executable"
assert_executable "${PROJECT_ROOT}/scripts/check-legal-notices.py" "Legal notice verifier is executable"
assert_executable "${PROJECT_ROOT}/scripts/generate-runtime-sbom.py" "Runtime SBOM generator is executable"
assert_executable "${PROJECT_ROOT}/scripts/generate-deployer-sbom.py" "Deployer SBOM generator is executable"

assert_ignored ".env.local" "Local environment variants are ignored"
assert_not_ignored ".env.example" "Environment examples remain visible"
assert_ignored "vars.yml" "Sensitive installer variables are ignored"
assert_ignored "omt-client-arm64.tar.gz" "Generated ARM64 image archive is ignored"
assert_ignored ".build/native/probe" "Native build output is ignored"
assert_ignored "tests/.venv/pyvenv.cfg" "Python test environment is ignored"

assert_literal "${PROJECT_ROOT}/CMakeLists.txt" 'set(CMAKE_C_STANDARD 17)' "C17 is required"
assert_literal "${PROJECT_ROOT}/CMakeLists.txt" 'set(CMAKE_CXX_STANDARD 20)' "C++20 is required"
assert_literal "${PROJECT_ROOT}/CMakeLists.txt" '-Werror' "Native warnings fail the build"
assert_literal "${PROJECT_ROOT}/CMakeLists.txt" '_FORTIFY_SOURCE=3' "Release builds enable Fortify"
assert_literal "${PROJECT_ROOT}/CMakeLists.txt" '-fstack-protector-strong' "Release builds protect stacks"
assert_literal "${PROJECT_ROOT}/CMakeLists.txt" '-Wl,-z,relro,-z,now,-z,noexecstack' "Release links are hardened"
assert_literal "${PROJECT_ROOT}/tools/test-receiver.sh" 'OMT_ENABLE_SANITIZERS=ON' "Receiver gate enables sanitizers"

dependencies="${PROJECT_ROOT}/cmake/NativeDependencies.cmake"
for digest in \
    e9fff7467fb60f037e6708da18b25560649e4c63edc2a69bb871b960d9cbfbba \
    fecb33d33930e12ff53a34064e9d3a06c8f7c3e04408f14cd36c80e3faac863b \
    d9ec76cbe34db98eec3539fe2c899d26b0c837cb3eb466a56b0f109cabf658f7; do
    assert_literal "${dependencies}" "${digest}" "Native source archive is SHA-256 locked"
done
assert_literal "${dependencies}" 'URL_HASH "SHA256=' "CMake enforces archive hashes"
assert_literal "${PROJECT_ROOT}/src/native/deployer/ssh_client.cpp" 'known_hosts' "Deployer uses strict known-host verification"
assert_literal "${PROJECT_ROOT}/src/native/deployer/core.cpp" 'getrandom' "Linux deployer uses the OS CSPRNG"
assert_literal "${PROJECT_ROOT}/src/native/deployer/core.cpp" 'BCryptGenRandom' "Windows deployer uses the OS CSPRNG"
assert_literal "${PROJECT_ROOT}/src/native/deployer/ssh_client.cpp" 'max_remote_output' "SSH output is bounded"
assert_literal "${PROJECT_ROOT}/src/native/omt/include/omt/omt_wire.h" 'OMT_WIRE_VIDEO_MAX_SIZE' "OMT video payloads are bounded"
assert_literal "${PROJECT_ROOT}/third_party/omt/libvmx/src/thread_tasks.h" '512U * 1024U' "VMX worker stacks are bounded"

assert_contains "${PROJECT_ROOT}/Makefile" '^test-deployer:' "Make exposes test-deployer"
assert_contains "${PROJECT_ROOT}/Makefile" '^build-deployer:' "Make exposes build-deployer"
assert_literal "${PROJECT_ROOT}/scripts/test-local.sh" '"${PROJECT_ROOT}/scripts/check-deployer.sh" --publish' "Full suite publishes the native deployer"
assert_literal "${PROJECT_ROOT}/scripts/build-native-receiver.sh" 'CMAKE_TOOLCHAIN_FILE=/src/cmake/toolchains/alpine-aarch64.cmake' "Receiver uses the ARM64 CMake toolchain"
assert_literal "${PROJECT_ROOT}/cmake/toolchains/alpine-aarch64.cmake" 'aarch64-alpine-linux-musl' "Clang targets Alpine ARM64"
assert_literal "${PROJECT_ROOT}/deploy/Dockerfile" 'FROM scratch AS receiver-artifacts' "Receiver export omits its build toolchain"
assert_literal "${PROJECT_ROOT}/deploy/Dockerfile" '--require-hashes' "Container Python install is hash locked"
assert_literal "${PROJECT_ROOT}/deploy/Dockerfile" 'runtime-sbom.cdx.json' "Container emits a runtime SBOM"
assert_literal "${PROJECT_ROOT}/deploy/compose.yml" 'mem_limit: "${OMT_CONTAINER_MEMORY_LIMIT:-256m}"' "Runtime memory defaults to a 256 MiB cap"
assert_literal "${PROJECT_ROOT}/deploy/compose.yml" 'pids_limit: 64' "Runtime process count is bounded"
assert_literal "${PROJECT_ROOT}/deploy/manifest-v3.txt" 'version=3' "Native deployment manifest is versioned"
assert_literal "${PROJECT_ROOT}/src/native/deployer/deployment.cpp" 'tonistiigi/binfmt@sha256:400a4873b838d1b89194d982c45e5fb3cda4593fbfd7e08a02e76b03b21166f0' "Deployer pins its privileged emulator image"
assert_literal "${PROJECT_ROOT}/scripts/security-scan.sh" '--skip-files vars.yml' "Security scan does not inspect sensitive vars.yml"
assert_executable "${PROJECT_ROOT}/scripts/install-arm64-emulation.sh" "ARM64 emulation bootstrap is executable"
assert_literal "${PROJECT_ROOT}/scripts/install-arm64-emulation.sh" 'docker.io/tonistiigi/binfmt@sha256:400a4873b838d1b89194d982c45e5fb3cda4593fbfd7e08a02e76b03b21166f0' "ARM64 bootstrap pins its emulator image"
assert_literal "${PROJECT_ROOT}/scripts/test-local.sh" 'export REQUIRE_ARM64_BUILD=1' "Full local gate requires the ARM64 builder"

assert_literal "${PYTHON_AUDIT}" 'run_audit audit_hash_locked "requirements/runtime.txt"' "Python audit covers runtime dependencies"
assert_literal "${PYTHON_AUDIT}" 'run_audit audit_pinned_no_deps "tests/requirements-dev.txt"' "Python audit covers development dependencies"
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
