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
       -path "${PROJECT_ROOT}/build" -prune -o \
       -path "${PROJECT_ROOT}/.git" -prune -o \
       \( -name '*.cs' -o -name '*.csproj' -o -name '*.sln' -o -name '*.slnx' -o -name '*.axaml' \) \
       -print -quit | grep -q .; then
    fail "Repository contains no C#/.NET project source"
else
    pass "Repository contains no C#/.NET project source"
fi
if find "${PROJECT_ROOT}" -path "${PROJECT_ROOT}/.build" -prune -o \
       -path "${PROJECT_ROOT}/build" -prune -o \
       -path "${PROJECT_ROOT}/.git" -prune -o \
       \( -name '*.cpp' -o -name '*.hpp' -o -name '*.cc' -o -name '*.cxx' \
          -o -name '*.c++' -o -name '*.hh' -o -name '*.hxx' \) \
       -print -quit | grep -q .; then
    fail "Repository contains no C++ source"
else
    pass "Repository contains no C++ source"
fi
cxx_pattern='LANGUAGES[[:space:]].*CXX|CMAKE_CXX|clang\+\+|(^|[^[:alnum:]_])g\+\+|libstdc\+\+|static-libstdc\+\+'
cxx_build_refs="$(grep -rInE "${cxx_pattern}" \
    "${PROJECT_ROOT}/CMakeLists.txt" "${PROJECT_ROOT}/cmake" \
    "${PROJECT_ROOT}/scripts" "${PROJECT_ROOT}/tools" \
    --include='CMakeLists.txt' --include='*.cmake' --include='*.sh' \
    --exclude-dir=.build || true)"
# grep applies --include to explicitly named files too, so the Dockerfile needs
# its own pass or it is silently skipped by the filters above. The runtime stage
# deletes libstdc++, so a bare removal path is exempt; the assertion below keeps
# that exemption from being used to install the runtime back in.
cxx_build_refs+="$(grep -InE "${cxx_pattern}" "${PROJECT_ROOT}/deploy/Dockerfile" \
    | grep -vE ':[[:space:]]*/usr/lib/libstdc\+\+\.so\*[[:space:]]*\\?$' || true)"
if [[ -z "${cxx_build_refs}" ]]; then
    pass "Native builds cannot invoke a C++ toolchain or runtime"
else
    printf '%s\n' "${cxx_build_refs}" >&2
    fail "Native builds cannot invoke a C++ toolchain or runtime"
fi
assert_contains "${PROJECT_ROOT}/deploy/Dockerfile" '^[[:space:]]*/usr/lib/libstdc\+\+\.so\*[[:space:]]*\\?$' \
    "Runtime image deletes the C++ standard library"

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
assert_literal "${PROJECT_ROOT}/CMakeLists.txt" 'LANGUAGES C)' "Only the C language is enabled"
assert_literal "${PROJECT_ROOT}/CMakeLists.txt" '-Werror' "Native warnings fail the build"
assert_literal "${PROJECT_ROOT}/CMakeLists.txt" '_FORTIFY_SOURCE=3' "Release builds enable Fortify"
assert_literal "${PROJECT_ROOT}/CMakeLists.txt" '-fstack-protector-strong' "Release builds protect stacks"
assert_literal "${PROJECT_ROOT}/CMakeLists.txt" '-Wl,-z,relro,-z,now,-z,noexecstack' "Release links are hardened"
assert_literal "${PROJECT_ROOT}/tools/test-receiver.sh" 'OMT_ENABLE_SANITIZERS=ON' "Receiver gate enables sanitizers"

dependencies="${PROJECT_ROOT}/cmake/NativeDependencies.cmake"
for digest in \
    e9fff7467fb60f037e6708da18b25560649e4c63edc2a69bb871b960d9cbfbba \
    834bf30974a294e996f7b1222aa59f1eb4ee259bd8d7d7967e8a2fb213d82dde \
    d9ec76cbe34db98eec3539fe2c899d26b0c837cb3eb466a56b0f109cabf658f7; do
    assert_literal "${dependencies}" "${digest}" "Native source archive is SHA-256 locked"
done
assert_literal "${dependencies}" 'URL_HASH "SHA256=' "CMake enforces archive hashes"
assert_literal "${PROJECT_ROOT}/src/native/deployer/ssh_client.c" 'known_hosts' "Deployer uses strict known-host verification"
assert_literal "${PROJECT_ROOT}/src/native/deployer/core.c" 'getrandom' "Linux deployer uses the OS CSPRNG"
assert_literal "${PROJECT_ROOT}/src/native/deployer/core.c" 'BCryptGenRandom' "Windows deployer uses the OS CSPRNG"
assert_literal "${PROJECT_ROOT}/src/native/deployer/deployer.h" 'OMT_DEPLOYER_OUTPUT_LIMIT (4U * 1024U * 1024U)' "SSH output is bounded"
assert_literal "${PROJECT_ROOT}/src/native/omt/include/omt/omt_wire.h" 'OMT_WIRE_VIDEO_MAX_SIZE' "OMT video payloads are bounded"
assert_literal "${PROJECT_ROOT}/third_party/omt/libvmx/src/thread_tasks.c" '512U * 1024U' "VMX worker stacks are bounded"

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
assert_literal "${PROJECT_ROOT}/src/native/deployer/deployment.c" 'tonistiigi/binfmt@sha256:400a4873b838d1b89194d982c45e5fb3cda4593fbfd7e08a02e76b03b21166f0' "Deployer pins its privileged emulator image"
assert_literal "${PROJECT_ROOT}/scripts/security-scan.sh" '--skip-files vars.yml' "Security scan does not inspect sensitive vars.yml"
assert_executable "${PROJECT_ROOT}/scripts/install-arm64-emulation.sh" "ARM64 emulation bootstrap is executable"
assert_literal "${PROJECT_ROOT}/scripts/install-arm64-emulation.sh" 'docker.io/tonistiigi/binfmt@sha256:400a4873b838d1b89194d982c45e5fb3cda4593fbfd7e08a02e76b03b21166f0' "ARM64 bootstrap pins its emulator image"
assert_literal "${PROJECT_ROOT}/tests/integration/test_docker_build.sh" \
    'fail "ARM64 emulation is unavailable' "Container gate requires the ARM64 builder"

# Windows deployment application, cross-built from this Linux workstation.
assert_executable "${PROJECT_ROOT}/scripts/build-windows-deployer.sh" "Windows cross build is executable"
assert_executable "${PROJECT_ROOT}/scripts/verify-windows-deployer.sh" "Windows artifact verifier is executable"
assert_contains "${PROJECT_ROOT}/Makefile" '^build-windows-deployer:' "Make exposes build-windows-deployer"
assert_literal "${PROJECT_ROOT}/scripts/test-local.sh" \
    '"${PROJECT_ROOT}/scripts/build-windows-deployer.sh"' "Local gate cross-builds the Windows deployer"
assert_literal "${PROJECT_ROOT}/scripts/build-windows-deployer.sh" \
    'cmake/toolchains/windows-x86_64-mingw.cmake' "Windows build uses the pinned cross toolchain"
assert_literal "${PROJECT_ROOT}/cmake/toolchains/windows-x86_64-mingw.cmake" \
    'set(CMAKE_SYSTEM_NAME Windows)' "Cross toolchain targets Windows"
assert_literal "${PROJECT_ROOT}/cmake/toolchains/windows-x86_64-mingw.cmake" \
    '-static -static-libgcc' "Windows deployer links its runtime statically"
assert_literal "${PROJECT_ROOT}/CMakeLists.txt" \
    '-Wl,--dynamicbase;-Wl,--nxcompat;-Wl,--high-entropy-va' "Windows release links are hardened"
assert_literal "${PROJECT_ROOT}/scripts/install-dev-deps.sh" \
    'x86_64-w64-mingw32-gcc' "Install provisions the Windows C cross toolchain"
assert_executable "${PROJECT_ROOT}/scripts/install-trivy.sh" "Trivy bootstrap is executable"
assert_literal "${PROJECT_ROOT}/scripts/install-dev-deps.sh" \
    'install-trivy.sh' "Install provisions the security scanner"

# No gate may report a pass for work it did not do. A missing tool, an
# unregistered emulator, or a case that excuses itself has to fail the run.
skip_escapes="$(grep -rInE 'SKIP_RETURN_CODE|pytest\.(skip|mark\.skip|mark\.xfail)|SKIP\$\{NC\}|"SKIP:|SKIP: ' \
    "${PROJECT_ROOT}/tests" "${PROJECT_ROOT}/scripts" "${PROJECT_ROOT}/tools" \
    --include='*.sh' --include='*.py' --include='CMakeLists.txt' \
    --exclude-dir=.venv --exclude="$(basename -- "${BASH_SOURCE[0]}")" || true)"
if [[ -z "${skip_escapes}" ]]; then
    pass "No gate can skip a check instead of running it"
else
    printf '%s\n' "${skip_escapes}" >&2
    fail "No gate can skip a check instead of running it"
fi
assert_literal "${PROJECT_ROOT}/tests/conftest.py" 'session.exitstatus = 1' "Python suite fails on any excused case"

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
