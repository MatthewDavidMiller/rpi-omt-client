#!/bin/bash
# Structural checks for the checksum-locked Rust build and release gates.
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PASS=0 FAIL=0
pass(){ printf 'PASS: %s\n' "$1"; PASS=$((PASS+1)); }
fail(){ printf 'FAIL: %s\n' "$1" >&2; FAIL=$((FAIL+1)); }
literal(){ grep -Fq -- "$2" "$1" && pass "$3" || fail "$3"; }
executable(){ [[ -x "$1" ]] && pass "$2" || fail "$2"; }
absent(){ [[ ! -e "$1" ]] && pass "$2" || fail "$2"; }

echo "=== Rust Supply Chain Guardrail Tests ==="
absent "${PROJECT_ROOT}/CMakeLists.txt" "CMake entry point is absent"
absent "${PROJECT_ROOT}/src/native" "Former C native tree is absent"
"${PROJECT_ROOT}/scripts/check-no-c-sources.sh" >/dev/null && pass "Tracked C/C++ gate passes" || fail "Tracked C/C++ gate passes"
literal "${PROJECT_ROOT}/rust-toolchain.toml" 'channel = "1.97.1"' "Rust compiler is patch pinned"
literal "${PROJECT_ROOT}/Cargo.toml" 'edition = "2024"' "Workspace uses Rust 2024"
literal "${PROJECT_ROOT}/Cargo.toml" 'unsafe_code = "forbid"' "Workspace forbids unsafe by default"
literal "${PROJECT_ROOT}/Cargo.toml" 'panic = "abort"' "Release binaries abort on panic"
literal "${PROJECT_ROOT}/Cargo.lock" 'checksum = ' "Registry dependencies are checksum locked"
literal "${PROJECT_ROOT}/deny.toml" 'unknown-git = "deny"' "Unreviewed Git dependencies are denied"
literal "${PROJECT_ROOT}/deny.toml" 'name = "libssh2-sys"' "Legacy libssh2 is banned"
literal "${PROJECT_ROOT}/crates/omt-protocol/src/lib.rs" 'VIDEO_MAX_SIZE: usize = 10 * 1024 * 1024' "Video frames are bounded"
literal "${PROJECT_ROOT}/crates/vmx-decoder/src/lib.rs" 'WORKER_STACK_SIZE: usize = 512 * 1024' "VMX worker stacks are bounded"
# The migration plan allows unsafe in the VMX SIMD kernels and nowhere else, so
# the carve-out is pinned to that one file rather than left to review.
literal "${PROJECT_ROOT}/crates/vmx-decoder/Cargo.toml" 'unsafe_op_in_unsafe_fn = "deny"' "VMX unsafe is individually justified"
literal "${PROJECT_ROOT}/crates/vmx-decoder/src/idct/neon.rs" '#![allow(unsafe_code)]' "VMX SIMD kernel is the declared unsafe carve-out"
allowing_unsafe="$(grep -rl 'allow(unsafe_code)' "${PROJECT_ROOT}/crates" || true)"
if [[ "${allowing_unsafe}" == "${PROJECT_ROOT}/crates/vmx-decoder/src/idct/neon.rs" ]]; then
    pass "No crate outside the VMX SIMD kernel allows unsafe"
else
    fail "No crate outside the VMX SIMD kernel allows unsafe"
fi
literal "${PROJECT_ROOT}/crates/omt-receiver-core/src/lib.rs" 'Duration::from_millis(500)' "Status heartbeat remains 500 ms"
literal "${PROJECT_ROOT}/crates/omt-deployer-core/src/lib.rs" 'OUTPUT_LIMIT: usize = 4 * 1024 * 1024' "Deployer output is bounded"
literal "${PROJECT_ROOT}/crates/omt-deployer-core/src/lib.rs" 'pub enum ManagementAction' "Management actions are typed"
literal "${PROJECT_ROOT}/crates/omt-deployer-core/src/lib.rs" 'pbkdf2::<Hmac<Sha1>>' "WPA PSKs are derived locally"
literal "${PROJECT_ROOT}/crates/rpi-omt-deploy/src/main.rs" 'deny_unknown_fields' "Secrets input rejects unknown fields"
literal "${PROJECT_ROOT}/crates/rpi-omt-deployer/src/main.rs" 'include_str!' "GUI legal text is embedded"
literal "${PROJECT_ROOT}/scripts/generate-runtime-sbom.py" 'RUNTIME_ROOTS = ["omt-receiver"]' "Runtime SBOM is scoped to the receiver"
literal "${PROJECT_ROOT}/scripts/generate-deployer-sbom.py" 'DEPLOYER_ROOTS = ["rpi-omt-deploy", "rpi-omt-deployer"]' "Deployer SBOM is scoped to the deployer"
literal "${PROJECT_ROOT}/deploy/Dockerfile" 'rust:1.97.1-alpine3.23@${RUST_DIGEST}' "Container builder is compiler and digest pinned"
literal "${PROJECT_ROOT}/deploy/Dockerfile" 'FROM scratch AS receiver-artifacts' "Receiver export omits its toolchain"
literal "${PROJECT_ROOT}/deploy/Dockerfile" '--require-hashes' "Python install is hash locked"
literal "${PROJECT_ROOT}/deploy/Dockerfile" '--cargo-lock /tmp/Cargo.lock' "Runtime SBOM consumes the Rust lock"
literal "${PROJECT_ROOT}/deploy/compose.yml" 'mem_limit: "${OMT_CONTAINER_MEMORY_LIMIT:-256m}"' "Runtime memory is bounded"
literal "${PROJECT_ROOT}/deploy/compose.yml" 'pids_limit: 64' "Runtime process count is bounded"
executable "${PROJECT_ROOT}/tools/test-receiver.sh" "Rust receiver gate is executable"
executable "${PROJECT_ROOT}/scripts/check-deployer.sh" "Rust deployer gate is executable"
executable "${PROJECT_ROOT}/scripts/build-windows-deployer.sh" "Windows Rust gate is executable"
executable "${PROJECT_ROOT}/scripts/check-no-c-sources.sh" "No-C gate is executable"
literal "${PROJECT_ROOT}/Makefile" 'test-receiver:' "Make exposes test-receiver"
literal "${PROJECT_ROOT}/Makefile" 'test-deployer:' "Make exposes test-deployer"
literal "${PROJECT_ROOT}/scripts/security-scan.sh" '--skip-files vars.yml' "Security scan excludes vars.yml"

skip_escapes="$(grep -rInE 'SKIP_RETURN_CODE|pytest\.(skip|mark\.skip|mark\.xfail)|SKIP\$\{NC\}|"SKIP:' "${PROJECT_ROOT}/tests" "${PROJECT_ROOT}/scripts" "${PROJECT_ROOT}/tools" --include='*.sh' --include='*.py' --exclude-dir=.venv --exclude="$(basename -- "${BASH_SOURCE[0]}")" || true)"
[[ -z "${skip_escapes}" ]] && pass "No gate silently skips work" || { printf '%s\n' "${skip_escapes}" >&2; fail "No gate silently skips work"; }
literal "${PROJECT_ROOT}/tests/conftest.py" 'session.exitstatus = 1' "Python suite fails on excused cases"

echo
echo "Results: ${PASS} passed, ${FAIL} failed"
[[ ${FAIL} -eq 0 ]]
