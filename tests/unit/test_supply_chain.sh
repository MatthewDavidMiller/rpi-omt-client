#!/bin/bash
# Structural checks for the Rust build, its supply-chain policy, and the release gates.
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PASS=0 FAIL=0
pass(){ printf 'PASS: %s\n' "$1"; PASS=$((PASS+1)); }
fail(){ printf 'FAIL: %s\n' "$1" >&2; FAIL=$((FAIL+1)); }
literal(){ grep -Fq -- "$2" "$1" && pass "$3" || fail "$3"; }
executable(){ [[ -x "$1" ]] && pass "$2" || fail "$2"; }
absent(){ [[ ! -e "$1" ]] && pass "$2" || fail "$2"; }

echo "=== Supply Chain Guardrail Tests ==="
absent "${PROJECT_ROOT}/CMakeLists.txt" "CMake entry point is absent"
literal "${PROJECT_ROOT}/Cargo.toml" 'members = ["crates/*"]' "The Rust workspace is crates/"
literal "${PROJECT_ROOT}/scripts/check-rust.sh" "grep -q '^source = ' Cargo.lock" "No crate from outside the workspace can enter"
literal "${PROJECT_ROOT}/scripts/check-rust.sh" 'UNSAFE_CRATES=(omt-sys omt-vmx omt-fuzz)' "unsafe is confined to an allowlist of crates"
literal "${PROJECT_ROOT}/clippy.toml" 'std::io::Read::read_to_end' "Unbounded reads are disallowed in Rust"
rust_stable="$(sed -n 's/^channel = "\(.*\)"$/\1/p' "${PROJECT_ROOT}/rust-toolchain.toml")"
literal "${PROJECT_ROOT}/scripts/install-rust.sh" "RUST_STABLE=\"${rust_stable}\"" "The toolbox installs the pinned stable Rust"
executable "${PROJECT_ROOT}/scripts/check-rust.sh" "Rust static gate is executable"
"${PROJECT_ROOT}/scripts/check-no-c-sources.sh" >/dev/null && pass "No C sources; assembly stays with the decoder" || fail "No C sources; assembly stays with the decoder"
literal "${PROJECT_ROOT}/VERSION" '.' "The canonical version file exists"
literal "${PROJECT_ROOT}/Cargo.toml" 'pedantic = { level = "deny", priority = -1 }' "Clippy's pedantic lints are errors"
literal "${PROJECT_ROOT}/Cargo.toml" 'overflow-checks = true' "Release builds keep overflow checks"
literal "${PROJECT_ROOT}/scripts/rust-sanitized.sh" '-Zsanitizer=address' "Sanitizer builds exist"
literal "${PROJECT_ROOT}/scripts/check-rust.sh" 'python3 tests/abi/check_abi.py' "Foreign declarations are held to the platform headers"
literal "${PROJECT_ROOT}/clippy.toml" 'std::fs::read_to_string' "Unbounded file reads are disallowed"
literal "${PROJECT_ROOT}/crates/omt-protocol/src/lib.rs" 'pub const VIDEO_MAX_SIZE: u32 = 10 * 1024 * 1024;' "Video frames are bounded"
literal "${PROJECT_ROOT}/crates/omt-vmx/src/lib.rs" 'pub const WORKER_STACK_SIZE: usize = 128 * 1024;' "VMX worker stacks are bounded"
literal "${PROJECT_ROOT}/crates/omt-receiver-core/src/status.rs" 'pub const HEARTBEAT_MS: u64 = 500;' "Status heartbeat remains 500 ms"
literal "${PROJECT_ROOT}/crates/omt-deploy/src/sys.rs" 'pub const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;' "Deployer output is bounded"
literal "${PROJECT_ROOT}/crates/omt-deploy/src/core.rs" 'pub enum Action {' "Management actions are a fixed set"
literal "${PROJECT_ROOT}/crates/omt-deploy/src/core.rs" 'omt_crypto::pbkdf2_sha1(' "WPA PSKs are derived locally"
literal "${PROJECT_ROOT}/crates/rpi-omt-deploy/src/main.rs" 'only_keys(&SECRET_FIELDS)' "Secrets input rejects unknown fields"
literal "${PROJECT_ROOT}/crates/omt-capsule/build.rs" 'license: include_str!' "Deployer legal text is embedded"
literal "${PROJECT_ROOT}/crates/omt-deploy/src/ssh/kex.rs" 'kex-strict-s-v00@openssh.com' "Strict key exchange is required"
literal "${PROJECT_ROOT}/crates/omt-deploy/src/ssh/kex.rs" 'known_hosts::check(' "Deployer verifies OpenSSH known_hosts"
literal "${PROJECT_ROOT}/crates/omt-deploy/src/ops.rs" 'wpa_cli -i \"$iface\"' "Wi-Fi updates use wpa_cli"
literal "${PROJECT_ROOT}/scripts/build-openssl.sh" 'OPENSSL_SHA256="' "The Windows OpenSSL is checksum pinned"
literal "${PROJECT_ROOT}/scripts/generate-runtime-sbom.py" 'RUNTIME_ROOTS = ["omt-receiver", "omt-web"]' "Runtime SBOM is scoped to both appliance binaries"
literal "${PROJECT_ROOT}/scripts/generate-deployer-sbom.py" 'DEPLOYER_ROOTS = ["rpi-omt-deploy", "rpi-omt-deploy-gui"]' "Deployer SBOM is scoped to the deployer"
literal "${PROJECT_ROOT}/deploy/Dockerfile" 'alpine:3.23.5@${ALPINE_DIGEST} AS builder' "Container builder is digest pinned"
literal "${PROJECT_ROOT}/deploy/Dockerfile" 'FROM scratch AS runtime-artifacts' "Runtime export omits its toolchain"
literal "${PROJECT_ROOT}/deploy/Dockerfile" '--apk-installed /tmp/runtime-installed' "Runtime SBOM inventories the installed packages"
literal "${PROJECT_ROOT}/deploy/compose.yml" 'mem_limit: "${OMT_CONTAINER_MEMORY_LIMIT:-512m}"' "Runtime memory is bounded"
literal "${PROJECT_ROOT}/deploy/compose.yml" 'pids_limit: 64' "Runtime process count is bounded"
executable "${PROJECT_ROOT}/tools/test-receiver.sh" "Receiver gate is executable"
executable "${PROJECT_ROOT}/tools/test-web.sh" "Web gate is executable"
executable "${PROJECT_ROOT}/scripts/rust-sanitized.sh" "Sanitizer and fuzz gate is executable"
executable "${PROJECT_ROOT}/scripts/test-arm64.sh" "AArch64 suite gate is executable"
executable "${PROJECT_ROOT}/tests/abi/check_abi.py" "ABI probe is executable"
executable "${PROJECT_ROOT}/scripts/check-deployer.sh" "Deployer gate is executable"
executable "${PROJECT_ROOT}/scripts/build-windows-deployer.sh" "Windows deployer gate is executable"
executable "${PROJECT_ROOT}/scripts/build-openssl.sh" "Pinned OpenSSL build is executable"
executable "${PROJECT_ROOT}/tests/integration/test_ssh_client.sh" "SSH interop gate is executable"
executable "${PROJECT_ROOT}/scripts/publish-github-release.sh" "Local GitHub Release publisher is executable"
executable "${PROJECT_ROOT}/scripts/check-no-c-sources.sh" "Source-language gate is executable"
literal "${PROJECT_ROOT}/Makefile" 'test-receiver:' "Make exposes test-receiver"
literal "${PROJECT_ROOT}/Makefile" 'test-deployer:' "Make exposes test-deployer"
literal "${PROJECT_ROOT}/scripts/security-scan.sh" '--skip-files vars.yml' "Security scan excludes vars.yml"

executable "${PROJECT_ROOT}/scripts/check-supply-chain.sh" "Supply-chain gate is executable"
literal "${PROJECT_ROOT}/scripts/check-supply-chain.sh" 'ALLOWED_LIBRARY=' "Linked native libraries are allowlisted by a gate"
"${PROJECT_ROOT}/scripts/check-supply-chain.sh" >/dev/null && pass "Supply-chain gate passes" || fail "Supply-chain gate passes"

skip_escapes="$(grep -rInE 'SKIP_RETURN_CODE|pytest\.(skip|mark\.skip|mark\.xfail)|SKIP\$\{NC\}|"SKIP:' "${PROJECT_ROOT}/tests" "${PROJECT_ROOT}/scripts" "${PROJECT_ROOT}/tools" --include='*.sh' --include='*.py' --exclude-dir=.venv --exclude="$(basename -- "${BASH_SOURCE[0]}")" || true)"
[[ -z "${skip_escapes}" ]] && pass "No gate silently skips work" || { printf '%s\n' "${skip_escapes}" >&2; fail "No gate silently skips work"; }
literal "${PROJECT_ROOT}/tests/conftest.py" 'session.exitstatus = 1' "Python suite fails on excused cases"

echo
echo "Results: ${PASS} passed, ${FAIL} failed"
[[ ${FAIL} -eq 0 ]]
